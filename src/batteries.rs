//! Battery levels of wireless devices: keyboards, mice, headsets, controllers.
//!
//! What Windows' Bluetooth stack publishes, read from the PnP configuration manager. A device is a
//! *container* of device nodes, and its facts are spread over them:
//!
//! - the level (`DEVPKEY_Bluetooth_Battery`) sits on the LE device node (`BTHLE\DEV_…`) for a
//!   Bluetooth LE device, and on the hands-free node (`BTHENUM\{0000111E…}`) for a Classic headset;
//! - whether it is connected (`{83DA6326…} 15`) and its name and category sit on its root node
//!   (`BTHLE\DEV_…` or `BTHENUM\DEV_…`).
//!
//! A paired device out of range keeps its last level, days old: only connected ones are listed.
//!
//! Nothing polls the Bluetooth ones. [`Watch`] keeps a `DeviceWatcher` on the Bluetooth nodes
//! asking for those two properties; any change to them wakes its thread, which reads everything
//! again and reports it to the hub when it changed.
//!
//! The devices on a Logitech receiver ([`hidpp`]) and Razer headsets on theirs ([`razer`]) are
//! other sources: Windows knows nothing of them, so they are asked, and nothing says when their
//! level moves. More watchers wake the thread when such a receiver comes or goes; while one is
//! plugged in, it reads again every [`POLL`] as well.

mod hid;
mod hidpp;
mod razer;

use std::collections::HashMap;

use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceWatcher};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_Locate_DevNodeW,
    CM_GETIDLIST_FILTER_PRESENT, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_DeviceContainer_Category, DEVPKEY_Device_ContainerId, DEVPKEY_Device_FriendlyName, DEVPROPTYPE,
    DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING, DEVPROP_TYPE_STRING_LIST,
};
use windows::Win32::Foundation::{CloseHandle, DEVPROPKEY, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject, INFINITE};

use crate::hub::{Battery, BatteryKind};

/// `DEVPKEY_Bluetooth_Battery`: the level, a byte 0..=100.
const BATTERY: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x104E_A319_6EE2_4701_BD47_8DDB_F425_BBE5), pid: 2 };
/// Whether the device is connected now (`DEVPKEY_Device_IsConnected`, unbound in `windows`).
const CONNECTED: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x83DA_6326_97A6_4088_9453_A192_3F57_3B29), pid: 15 };

/// The same two, as the device watcher asks for them.
const WATCHED: [&str; 2] = ["{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2", "{83DA6326-97A6-4088-9453-A1923F573B29} 15"];
/// The Bluetooth nodes: every one the facts above sit on.
const WATCH_FILTER: &str = r#"System.Devices.DeviceInstanceId:~<"BTH""#;
/// The vendors whose receivers are asked: their HID interfaces are watched for one plugged in or out.
const RECEIVER_VENDORS: [u16; 2] = [hidpp::VENDOR, razer::VENDOR];

/// How long the thread lets a burst of changes settle before reading (a connection updates
/// several nodes, one after the other).
const SETTLE_MS: u32 = 400;
/// How often a receiver's devices are asked again: a level moves slowly, and a device switched
/// on shows up within this.
const POLL: std::time::Duration = std::time::Duration::from_secs(120);
/// After a change, how soon they are asked once more: a receiver plugged in (or switched to this
/// PC) finds its devices a few seconds after it appears.
const RECHECK: std::time::Duration = std::time::Duration::from_secs(5);

/// Every connected device that reports a battery, by name. Best-effort: empty on any failure.
pub fn read() -> Vec<Battery> {
    Sources::default().read().0
}

/// Where levels come from, and what they remember between reads.
#[derive(Default)]
struct Sources {
    logitech: hidpp::Cache,
}

impl Sources {
    /// Every level, and whether a receiver is plugged in (whose devices must be polled).
    fn read(&mut self) -> (Vec<Battery>, bool) {
        let mut list = unsafe { collect() }.unwrap_or_default();
        let mut polled = false;
        for receivers in [hidpp::read(&mut self.logitech), razer::read()] {
            polled |= receivers.is_some();
            list.extend(receivers.unwrap_or_default());
        }
        list.sort_by_key(|b| b.name.to_lowercase());
        (list, polled)
    }
}

/// What one node says about its device.
#[derive(Default)]
struct Facts {
    level: Option<u8>,
    connected: Option<bool>,
    name: Option<String>,
    category: Option<String>,
}

/// What one container's nodes said, gathered. The device's own node (its root) speaks for it:
/// its service nodes have names of their own ("Standard Serial over Bluetooth link (COM5)").
#[derive(Default)]
struct Gathered {
    level: Option<u8>,
    root: Option<Facts>,
    /// From the other nodes, when there is no root: connected if any is, the first name.
    connected: Option<bool>,
    name: Option<String>,
}

impl Gathered {
    fn add(&mut self, facts: Facts, root: bool) {
        if facts.level.is_some() {
            self.level = facts.level;
        }
        if root {
            self.root = Some(facts);
            return;
        }
        if let Some(connected) = facts.connected {
            self.connected = Some(self.connected.unwrap_or(false) || connected);
        }
        if self.name.is_none() {
            self.name = facts.name;
        }
    }

    /// The device, when it is connected and reports a level. A device that does not say whether it
    /// is connected (not Bluetooth) is taken as connected.
    fn battery(self, id: String) -> Option<Battery> {
        let root = self.root.unwrap_or_default();
        if root.connected.or(self.connected) == Some(false) {
            return None;
        }
        Some(Battery {
            id,
            name: root.name.or(self.name).unwrap_or_else(|| "Device".to_string()),
            kind: root.category.as_deref().map_or(BatteryKind::Other, BatteryKind::from_category),
            level: self.level?.min(100),
        })
    }
}

/// A device's own node, as the Bluetooth stack enumerates it (`BTHENUM\DEV_…`, `BTHLE\DEV_…`);
/// its services hang under it.
fn is_root(instance_id: &[u16]) -> bool {
    let id = String::from_utf16_lossy(instance_id).to_ascii_uppercase();
    id.starts_with(r"BTHENUM\DEV_") || id.starts_with(r"BTHLE\DEV_")
}

unsafe fn collect() -> Option<Vec<Battery>> {
    let mut by_container: HashMap<String, Gathered> = HashMap::new();
    for id in present_ids()? {
        let wide: Vec<u16> = id.iter().copied().chain(std::iter::once(0)).collect();
        let mut devinst = 0u32;
        if CM_Locate_DevNodeW(&mut devinst, PCWSTR(wide.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
            continue;
        }
        let root = is_root(&id);
        let level = byte_prop(devinst, &BATTERY);
        let connected = bool_prop(devinst, &CONNECTED);
        if !root && level.is_none() && connected.is_none() {
            continue;
        }
        let Some(container) = guid_prop(devinst, &DEVPKEY_Device_ContainerId) else { continue };
        let facts = Facts {
            level,
            connected,
            name: string_prop(devinst, &DEVPKEY_Device_FriendlyName),
            category: if root { string_prop(devinst, &DEVPKEY_DeviceContainer_Category) } else { None },
        };
        by_container.entry(guid_string(&container)).or_default().add(facts, root);
    }
    Some(by_container.into_iter().filter_map(|(id, gathered)| gathered.battery(id)).collect())
}

/// Every present device's instance id.
unsafe fn present_ids() -> Option<Vec<Vec<u16>>> {
    // A double-NUL-terminated multi-string; a device arriving between the two calls makes the
    // second fail, so try again once.
    for _ in 0..2 {
        let mut len = 0u32;
        if CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR::null(), CM_GETIDLIST_FILTER_PRESENT) != CR_SUCCESS || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_ID_ListW(PCWSTR::null(), &mut buf, CM_GETIDLIST_FILTER_PRESENT) == CR_SUCCESS {
            return Some(buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(<[u16]>::to_vec).collect());
        }
    }
    None
}

/// A devnode property's raw bytes and type.
unsafe fn prop(devinst: u32, key: &DEVPROPKEY) -> Option<(DEVPROPTYPE, Vec<u8>)> {
    let mut ptype = DEVPROPTYPE(0);
    let mut size = 0u32;
    // Sized first: CR_BUFFER_SMALL, with the size.
    let _ = CM_Get_DevNode_PropertyW(devinst, key, &mut ptype, None, &mut size, 0);
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let ret = CM_Get_DevNode_PropertyW(devinst, key, &mut ptype, Some(buf.as_mut_ptr()), &mut size, 0);
    (ret == CR_SUCCESS).then(|| {
        buf.truncate(size as usize);
        (ptype, buf)
    })
}

unsafe fn byte_prop(devinst: u32, key: &DEVPROPKEY) -> Option<u8> {
    match prop(devinst, key)? {
        (DEVPROP_TYPE_BYTE, bytes) => bytes.first().copied(),
        _ => None,
    }
}

unsafe fn bool_prop(devinst: u32, key: &DEVPROPKEY) -> Option<bool> {
    match prop(devinst, key)? {
        // DEVPROP_BOOLEAN: 0 is false, 0xFF true.
        (DEVPROP_TYPE_BOOLEAN, bytes) => bytes.first().map(|&b| b != 0),
        _ => None,
    }
}

unsafe fn guid_prop(devinst: u32, key: &DEVPROPKEY) -> Option<GUID> {
    match prop(devinst, key)? {
        (DEVPROP_TYPE_GUID, bytes) if bytes.len() >= 16 => Some(GUID::from_u128(u128::from_be_bytes(guid_bytes(&bytes)))),
        _ => None,
    }
}

/// A GUID's in-memory layout (little-endian fields) as the big-endian `u128` `GUID::from_u128` takes.
fn guid_bytes(raw: &[u8]) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    b[4..6].copy_from_slice(&[raw[5], raw[4]]);
    b[6..8].copy_from_slice(&[raw[7], raw[6]]);
    b[8..16].copy_from_slice(&raw[8..16]);
    b
}

/// A string, or a string list's first entry (a container's category is a list).
unsafe fn string_prop(devinst: u32, key: &DEVPROPKEY) -> Option<String> {
    let (ptype, bytes) = prop(devinst, key)?;
    if ptype != DEVPROP_TYPE_STRING && ptype != DEVPROP_TYPE_STRING_LIST {
        return None;
    }
    let wide: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let first = wide.split(|&c| c == 0).next()?;
    let s = String::from_utf16_lossy(first).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Format a GUID as an upper-case `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` string, matching
/// what `PropVariantToStringAlloc` yields for an audio endpoint's own container id.
fn guid_string(g: &GUID) -> String {
    let d = g.data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// An event handle, closed on drop. Raw, so it crosses to the watcher's callbacks.
struct Event(isize);

impl Event {
    /// Auto-reset.
    fn new() -> Option<Self> {
        unsafe { CreateEventW(None, false, false, None) }.ok().map(|h| Event(h.0 as isize))
    }

    /// Manual-reset, as overlapped I/O wants.
    fn manual() -> Option<Self> {
        unsafe { CreateEventW(None, true, false, None) }.ok().map(|h| Event(h.0 as isize))
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}

/// The battery source: while it lives, the hub hears every change in the levels.
pub struct Watch {
    stop: std::sync::Arc<Event>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    /// Report the levels now, then on every change. `None` if the thread would not start.
    pub fn spawn() -> Option<Self> {
        let stop = std::sync::Arc::new(Event::new()?);
        let theirs = stop.clone();
        let join = std::thread::Builder::new()
            .name("batteries".into())
            .spawn(move || serve(&theirs, |batteries| crate::hub::emit(crate::hub::Event::Batteries(batteries))))
            .ok()?;
        Some(Watch { stop, join: Some(join) })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.handle());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The watch thread: report, then wait for a change (or `stop`), let it settle, and report again
/// if anything moved. While a receiver is plugged in, the wait also ends on a timer. `report`
/// hears each new list.
fn serve(stop: &Event, mut report: impl FnMut(Vec<Battery>)) {
    // The watcher's callbacks come on the thread pool; this thread only needs COM for creating it.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let Some(changed) = Event::new().map(std::sync::Arc::new) else { return };
    let mut watchers = Vec::new();
    let receivers = RECEIVER_VENDORS.map(|vendor| (hid::watch_filter(vendor), DeviceInformationKind::DeviceInterface, &[][..]));
    for (filter, kind, props) in std::iter::once((WATCH_FILTER.to_string(), DeviceInformationKind::Device, &WATCHED[..])).chain(receivers) {
        match watch(&filter, kind, props, changed.clone()) {
            Ok(watcher) => watchers.push(watcher),
            Err(e) => eprintln!("batteries: cannot watch {filter} for changes ({e:#})"),
        }
    }
    let mut sources = Sources::default();
    let (mut last, mut polled) = sources.read();
    report(last.clone());
    let mut soon = true;
    loop {
        let wait = match (polled, soon) {
            (false, _) => INFINITE,
            (true, true) => RECHECK.as_millis() as u32,
            (true, false) => POLL.as_millis() as u32,
        };
        let woke = unsafe { WaitForMultipleObjects(&[stop.handle(), changed.handle()], false, wait) };
        if woke == windows::Win32::Foundation::WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
            // Let the rest of the burst land; stop still ends it at once.
            if unsafe { WaitForSingleObject(stop.handle(), SETTLE_MS) } == WAIT_OBJECT_0 {
                break;
            }
            soon = true;
        } else if woke == WAIT_TIMEOUT {
            soon = false;
        } else {
            break;
        }
        let (now, receivers) = sources.read();
        polled = receivers;
        if now != last {
            last = now;
            report(last.clone());
        }
    }
    for watcher in watchers {
        let _ = watcher.Stop();
    }
}

/// A watcher that sets `changed` on any change to the devices `filter` picks, or to `props` of
/// theirs. Its first enumeration (every one, once) is let through as well: one read, after it
/// settles.
fn watch(filter: &str, kind: DeviceInformationKind, props: &[&str], changed: std::sync::Arc<Event>) -> windows::core::Result<DeviceWatcher> {
    let props: Vec<HSTRING> = props.iter().map(|p| HSTRING::from(*p)).collect();
    let props = windows_collections::IIterable::<HSTRING>::from(props);
    let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(&HSTRING::from(filter), &props, kind)?;
    let poke = move || unsafe {
        WAKES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = SetEvent(changed.handle());
    };
    let on_added = poke.clone();
    watcher.Added(&TypedEventHandler::new(move |_, _| {
        on_added();
        Ok(())
    }))?;
    let on_updated = poke.clone();
    watcher.Updated(&TypedEventHandler::new(move |_, _| {
        on_updated();
        Ok(())
    }))?;
    watcher.Removed(&TypedEventHandler::new(move |_, _| {
        poke();
        Ok(())
    }))?;
    watcher.Start()?;
    Ok(watcher)
}

/// Watcher callbacks so far, for the dev trace.
static WAKES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `--batteries [secs]`: the levels now, then every change for `secs`, with how many watcher
/// callbacks led to it.
#[cfg(feature = "dev")]
pub fn trace(secs: u64) {
    let Some(stop) = Event::new().map(std::sync::Arc::new) else { return };
    let theirs = stop.clone();
    let start = std::time::Instant::now();
    let thread = std::thread::spawn(move || {
        serve(&theirs, |list| {
            let wakes = WAKES.load(std::sync::atomic::Ordering::Relaxed);
            println!("[{:6.1}s] {} device(s), {wakes} watcher callback(s) so far", start.elapsed().as_secs_f32(), list.len());
            for b in list {
                println!("  {:<28} {:>3}%  {:<8?} {}", b.name, b.level, b.kind, b.id);
            }
        })
    });
    std::thread::sleep(std::time::Duration::from_secs(secs));
    unsafe {
        let _ = SetEvent(stop.handle());
    }
    let _ = thread.join();
    println!("done: {} watcher callback(s) in {secs} s", WAKES.load(std::sync::atomic::Ordering::Relaxed));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(level: Option<u8>, connected: Option<bool>, name: &str, category: Option<&str>) -> Facts {
        Facts { level, connected, name: Some(name.into()), category: category.map(Into::into) }
    }

    /// A Classic headset: the level on its hands-free node, a serial port also saying it is
    /// connected, and the root naming it.
    #[test]
    fn the_root_names_the_device_whatever_its_services_say() {
        let mut g = Gathered::default();
        g.add(facts(Some(50), None, "Jabra Evolve 65 Hands-Free AG", None), false);
        g.add(facts(None, Some(true), "Standard Serial over Bluetooth link (COM5)", None), false);
        g.add(facts(None, Some(true), "Jabra Evolve 65", Some("Communication.Headset.Bluetooth")), true);
        let b = g.battery("{J}".into()).unwrap();
        assert_eq!((b.name.as_str(), b.kind, b.level), ("Jabra Evolve 65", BatteryKind::Headset, 50));
    }

    #[test]
    fn a_device_out_of_range_keeps_its_old_level_but_is_not_listed() {
        let mut g = Gathered::default();
        g.add(facts(Some(50), None, "Hands-Free AG", None), false);
        g.add(facts(None, Some(false), "Jabra Evolve 65", None), true);
        assert!(g.battery("{J}".into()).is_none());
    }

    #[test]
    fn a_device_without_a_level_is_not_listed() {
        let mut g = Gathered::default();
        g.add(facts(None, Some(true), "Phone", Some("Communication.Phone")), true);
        assert!(g.battery("{P}".into()).is_none());
    }

    #[test]
    fn roots_are_told_from_services() {
        let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(is_root(&wide(r"BTHLE\DEV_DFCDB3841EEB\7&294FA41&0&DFCDB3841EEB")));
        assert!(is_root(&wide(r"BTHENUM\DEV_70BF9244F7AB\7&194A43A9&0&BLUETOOTHDEVICE_70BF9244F7AB")));
        assert!(!is_root(&wide(r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&00010067_PID&094E\7&1")));
        assert!(!is_root(&wide(r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&02046D")));
    }

    #[test]
    fn a_guid_reads_back_from_its_bytes_as_windows_prints_it() {
        let g = GUID::from_u128(0x8B1C_8F60_1714_5936_8857_5330_03A0_D008);
        let raw: [u8; 16] = unsafe { std::mem::transmute(g) };
        let back = GUID::from_u128(u128::from_be_bytes(guid_bytes(&raw)));
        assert_eq!(guid_string(&back), "{8B1C8F60-1714-5936-8857-533003A0D008}");
    }
}
