//! The devices paired to a Logitech receiver (LIGHTSPEED, Unifying, Bolt), through HID++ 2.0.
//!
//! Windows knows nothing of their batteries: each device is asked, over the receiver's vendor
//! collections (usage page `FF00`: usage 1 carries the 7-byte "short" reports, usage 2 the 20-byte
//! "long" ones). A request goes out short; its reply comes back short or long:
//!
//! ```text
//! request  10 <device> <feature index> <function << 4 | software id> <3 parameter bytes>
//! reply    11 <device> <feature index> <the same byte> <16 bytes>
//! error    10 <device> 8F <feature index> <the same byte> <code>   (HID++ 1.0: none there, or asleep)
//!          11 <device> FF <feature index> <the same byte> <code>   (HID++ 2.0)
//! ```
//!
//! Features are numbered per device: the root feature (index 0) gives a feature id's index.
//! Devices are 1..=6 on a receiver. Logitech devices on Bluetooth are not here: Windows reports
//! those itself (the parent module).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, FILE_FLAG_OVERLAPPED};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{WaitForMultipleObjects, WaitForSingleObject};

use super::hid::{self, Collection, Handle};
use super::Event;
use crate::hub::{Battery, BatteryKind};

pub(super) const VENDOR: u16 = 0x046D;
/// The receiver's HID++ collections.
const VENDOR_PAGE: u16 = 0xFF00;
const SHORT: u16 = 1;
const LONG: u16 = 2;

/// Feature ids.
const DEVICE_NAME: u16 = 0x0005;
const BATTERY_STATUS: u16 = 0x1000;
const BATTERY_VOLTAGE: u16 = 0x1001;
const UNIFIED_BATTERY: u16 = 0x1004;
/// The battery features, the best first.
const BATTERIES: [u16; 3] = [UNIFIED_BATTERY, BATTERY_STATUS, BATTERY_VOLTAGE];

/// Ours, in each request's function byte, so replies to other software (G HUB) are told apart.
const SOFTWARE_ID: u8 = 0x0A;
/// A device answers in a few milliseconds; the receiver answers at once for one that is not there.
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);

/// What a device said of itself, kept: its name and features do not change.
#[derive(Clone)]
struct Known {
    name: String,
    kind: BatteryKind,
    /// The battery feature it has, and its index; `None` when it has none.
    battery: Option<(u16, u8)>,
}

/// The devices met so far, by battery id.
#[derive(Default)]
pub(super) struct Cache(HashMap<String, Known>);

/// Every connected device on every receiver plugged in that reports a battery; `None` when no
/// receiver is plugged in.
pub(super) fn read(cache: &mut Cache) -> Option<Vec<Battery>> {
    let receivers = receivers();
    if receivers.is_empty() {
        return None;
    }
    let mut list = Vec::new();
    for (key, (short, long)) in receivers {
        let Some(mut port) = (unsafe { Port::open(&short, long.as_ref()) }) else { continue };
        for device in 1..=6u8 {
            list.extend(unsafe { port.battery(&key, device, cache) });
        }
    }
    Some(list)
}

/// The receivers plugged in, by interface: their short collection, and their long one.
fn receivers() -> HashMap<String, (Collection, Option<Collection>)> {
    let (mut shorts, mut longs) = (HashMap::new(), HashMap::new());
    for collection in hid::collections(VENDOR) {
        if collection.is(VENDOR_PAGE, SHORT) {
            shorts.insert(collection.interface(), collection);
        } else if collection.is(VENDOR_PAGE, LONG) {
            longs.insert(collection.interface(), collection);
        }
    }
    shorts.into_iter().map(|(key, short)| {
        let long = longs.remove(&key);
        (key, (short, long))
    }).collect()
}

/// One collection, opened for overlapped reads, with a read kept under way.
struct Pipe {
    handle: Handle,
    done: Event,
    /// Boxed: the system writes to both until the read completes, wherever the pipe moves.
    overlapped: Box<OVERLAPPED>,
    buf: Box<[u8]>,
    pending: bool,
}

impl Pipe {
    unsafe fn open(collection: &Collection) -> Option<Self> {
        Some(Pipe {
            handle: Handle::open(&collection.path, GENERIC_READ.0 | GENERIC_WRITE.0, FILE_FLAG_OVERLAPPED)?,
            done: Event::manual()?,
            overlapped: Box::default(),
            buf: vec![0u8; collection.caps.InputReportByteLength.max(1) as usize].into_boxed_slice(),
            pending: false,
        })
    }

    /// Have a read under way; false if the collection is gone.
    unsafe fn arm(&mut self) -> bool {
        if self.pending {
            return true;
        }
        *self.overlapped = OVERLAPPED { hEvent: self.done.handle(), ..Default::default() };
        match ReadFile(self.handle.0, Some(&mut self.buf), None, Some(&mut *self.overlapped)) {
            // Done at once: the event is set all the same.
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(_) => return false,
        }
        self.pending = true;
        true
    }

    /// The report the read brought, once its event is set.
    unsafe fn take(&mut self) -> Option<Vec<u8>> {
        self.pending = false;
        let mut n = 0u32;
        GetOverlappedResult(self.handle.0, &*self.overlapped, &mut n, false).ok()?;
        Some(self.buf[..n as usize].to_vec())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // The read must end before its buffers go (the handle closes after this).
        if self.pending {
            unsafe {
                let _ = CancelIoEx(self.handle.0, Some(&*self.overlapped));
                let mut n = 0u32;
                let _ = GetOverlappedResult(self.handle.0, &*self.overlapped, &mut n, true);
            }
        }
    }
}

/// A receiver, open: requests go out on its short collection, replies come on either.
struct Port {
    short: Pipe,
    long: Option<Pipe>,
    /// The short output report's length (7).
    out_len: usize,
    written: Event,
}

impl Port {
    unsafe fn open(short: &Collection, long: Option<&Collection>) -> Option<Self> {
        Some(Port {
            short: Pipe::open(short)?,
            long: long.and_then(|long| Pipe::open(long)),
            out_len: (short.caps.OutputReportByteLength as usize).max(7),
            written: Event::manual()?,
        })
    }

    /// The device's battery, when it is connected and has one.
    unsafe fn battery(&mut self, receiver: &str, device: u8, cache: &mut Cache) -> Option<Battery> {
        let id = format!("HIDPP\\{receiver}\\{device}");
        // Off, asleep or never paired: the receiver answers for it, with an error.
        self.request(device, 0, 1, &[0, 0, 0x5A])?;
        let known = match cache.0.get(&id) {
            Some(known) => known.clone(),
            None => {
                let known = self.discover(device)?;
                cache.0.insert(id.clone(), known.clone());
                known
            }
        };
        let (feature, index) = known.battery?;
        match self.level(device, feature, index) {
            Some(level) => Some(Battery { id, name: known.name, kind: known.kind, level }),
            None => {
                // Another device paired in its place, maybe: learn it again next time.
                cache.0.remove(&id);
                None
            }
        }
    }

    /// Its name, kind and battery feature; `None` if it stopped answering.
    unsafe fn discover(&mut self, device: u8) -> Option<Known> {
        let mut known = Known { name: "Logitech device".to_string(), kind: BatteryKind::Other, battery: None };
        let names = self.feature(device, DEVICE_NAME)?;
        if names != 0 {
            if let Some(name) = self.name(device, names) {
                known.name = name;
            }
            if let Some(reply) = self.request(device, names, 2, &[]) {
                known.kind = kind(reply[4]);
            }
        }
        for feature in BATTERIES {
            let index = self.feature(device, feature)?;
            if index != 0 {
                known.battery = Some((feature, index));
                break;
            }
        }
        Some(known)
    }

    /// A feature's index on the device: 0 when it lacks it, `None` if it did not answer.
    unsafe fn feature(&mut self, device: u8, id: u16) -> Option<u8> {
        let [hi, lo] = id.to_be_bytes();
        self.request(device, 0, 0, &[hi, lo]).map(|reply| reply[4])
    }

    /// `DEVICE_NAME`: its length, then the name in pieces.
    unsafe fn name(&mut self, device: u8, index: u8) -> Option<String> {
        let len = self.request(device, index, 0, &[])?[4] as usize;
        let mut bytes = Vec::with_capacity(len);
        while bytes.len() < len {
            let reply = self.request(device, index, 1, &[bytes.len() as u8])?;
            let piece = &reply[4..reply.len().min(4 + len - bytes.len())];
            if piece.is_empty() {
                break;
            }
            bytes.extend_from_slice(piece);
        }
        let name = String::from_utf8_lossy(&bytes).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
        (!name.is_empty()).then_some(name)
    }

    /// The level, 0..=100, as its battery feature tells it.
    unsafe fn level(&mut self, device: u8, feature: u16, index: u8) -> Option<u8> {
        let level = match feature {
            // get_status: the percentage, else only a coarse level when it has no gauge.
            UNIFIED_BATTERY => {
                let reply = self.request(device, index, 1, &[])?;
                if reply[4] != 0 {
                    reply[4]
                } else {
                    coarse_level(reply[5])
                }
            }
            // get_battery_level_status: the percentage.
            BATTERY_STATUS => self.request(device, index, 0, &[])?[4],
            // get_battery_voltage: millivolts, big-endian.
            _ => {
                let reply = self.request(device, index, 0, &[])?;
                voltage_percent(u16::from_be_bytes([reply[4], reply[5]]))
            }
        };
        Some(level.min(100))
    }

    /// Send a request and wait for its reply (at least 5 bytes); `None` on an error or silence.
    unsafe fn request(&mut self, device: u8, index: u8, function: u8, params: &[u8]) -> Option<Vec<u8>> {
        let tag = (function << 4) | SOFTWARE_ID;
        let mut msg = vec![0u8; self.out_len];
        msg[..4].copy_from_slice(&[0x10, device, index, tag]);
        for (slot, p) in msg[4..].iter_mut().zip(params) {
            *slot = *p;
        }
        if !self.short.arm() || self.long.as_mut().is_some_and(|l| !l.arm()) {
            return None;
        }
        self.write(&msg)?;
        let mut events: Vec<HANDLE> = vec![self.short.done.handle()];
        events.extend(self.long.as_ref().map(|l| l.done.handle()));
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
            if left == 0 {
                return None;
            }
            let woke = WaitForMultipleObjects(&events, false, left).0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            let pipe = match woke {
                0 => &mut self.short,
                1 => self.long.as_mut()?,
                _ => return None,
            };
            let report = pipe.take()?;
            if !pipe.arm() {
                return None;
            }
            match answers(&report, device, index, tag) {
                Some(true) => return Some(report),
                Some(false) => return None,
                None => {}
            }
        }
    }

    unsafe fn write(&mut self, msg: &[u8]) -> Option<()> {
        let handle = self.short.handle.0;
        let mut overlapped = OVERLAPPED { hEvent: self.written.handle(), ..Default::default() };
        match WriteFile(handle, Some(msg), None, Some(&mut overlapped)) {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(_) => return None,
        }
        let mut n = 0u32;
        if WaitForSingleObject(self.written.handle(), REPLY_TIMEOUT.as_millis() as u32) != WAIT_OBJECT_0 {
            // `overlapped` lives on this stack: the write must end before it does.
            let _ = CancelIoEx(handle, Some(&overlapped));
            let _ = GetOverlappedResult(handle, &overlapped, &mut n, true);
            return None;
        }
        GetOverlappedResult(handle, &overlapped, &mut n, false).ok()
    }
}

/// Whether a report answers the request: `Some(true)` its reply, `Some(false)` an error about it,
/// `None` something else (another request's reply, a notification).
fn answers(report: &[u8], device: u8, index: u8, tag: u8) -> Option<bool> {
    if report.len() < 5 || report[1] != device {
        return None;
    }
    if report[2] == index && report[3] == tag {
        return Some(true);
    }
    if (report[2] == 0x8F || report[2] == 0xFF) && report[3] == index && report[4] == tag {
        return Some(false);
    }
    None
}

/// `DEVICE_NAME`'s device type.
fn kind(device_type: u8) -> BatteryKind {
    match device_type {
        0 | 2 => BatteryKind::Keyboard,
        3..=5 => BatteryKind::Mouse,
        8 => BatteryKind::Headset,
        10..=12 => BatteryKind::Gamepad,
        14 => BatteryKind::Speaker,
        _ => BatteryKind::Other,
    }
}

/// `UNIFIED_BATTERY`'s level bits (critical 1, low 2, good 4, full 8) as a percentage.
fn coarse_level(bits: u8) -> u8 {
    match bits {
        b if b & 8 != 0 => 100,
        b if b & 4 != 0 => 60,
        b if b & 2 != 0 => 20,
        b if b & 1 != 0 => 5,
        _ => 0,
    }
}

/// A Li-ion cell's charge from its voltage (mV), the discharge curve Solaar uses for
/// `BATTERY_VOLTAGE`, read linearly between its points.
fn voltage_percent(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 13] = [
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3811, 50),
        (3778, 40),
        (3751, 30),
        (3717, 20),
        (3671, 10),
        (3646, 5),
        (3579, 2),
        (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for pair in CURVE.windows(2) {
        let ((hi_mv, hi_pct), (lo_mv, lo_pct)) = (pair[0], pair[1]);
        if mv >= lo_mv {
            let t = f32::from(mv - lo_mv) / f32::from(hi_mv - lo_mv);
            return (f32::from(lo_pct) + t * f32::from(hi_pct - lo_pct)).round() as u8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the G502 LIGHTSPEED answered, and errors the receiver gave for empty slots.
    #[test]
    fn replies_are_told_from_errors_and_other_traffic() {
        let tag = SOFTWARE_ID;
        assert_eq!(answers(&[0x11, 1, 6, tag, 0x0E, 0x64, 0, 0], 1, 6, tag), Some(true));
        assert_eq!(answers(&[0x10, 2, 0x8F, 0, 0x1A, 8, 0], 2, 0, 0x1A), Some(false));
        assert_eq!(answers(&[0x11, 1, 0xFF, 6, tag, 5, 0], 1, 6, tag), Some(false));
        // G HUB's own polling, another device's reply.
        assert_eq!(answers(&[0x11, 1, 7, 0x3F, 0, 0, 0], 1, 6, tag), None);
        assert_eq!(answers(&[0x11, 2, 6, tag, 0, 0, 0], 1, 6, tag), None);
    }

    #[test]
    fn voltage_reads_along_the_curve() {
        assert_eq!(voltage_percent(4200), 100);
        assert_eq!(voltage_percent(3811), 50);
        assert_eq!(voltage_percent(3684), 13);
        assert_eq!(voltage_percent(3400), 0);
    }

    #[test]
    fn coarse_levels_take_the_highest_bit() {
        assert_eq!(coarse_level(0b1100), 100);
        assert_eq!(coarse_level(0b0010), 20);
        assert_eq!(coarse_level(0), 0);
    }

    #[test]
    fn device_types_map_to_glyphs() {
        assert_eq!(kind(0), BatteryKind::Keyboard);
        assert_eq!(kind(3), BatteryKind::Mouse);
        assert_eq!(kind(8), BatteryKind::Headset);
        assert_eq!(kind(7), BatteryKind::Other);
    }
}
