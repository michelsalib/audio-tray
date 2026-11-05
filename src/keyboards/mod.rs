//! A keyboard layout per physical keyboard: which keyboard is typing, and the layouts to switch
//! between. See `docs/KEYBOARDS.md`.
//!
//! - **Device identity only.** Raw Input is read with `RID_HEADER`, for `hDevice`; the key is never
//!   read, stored or logged (decision 2).
//! - **A layout is its full `HKL`**, never its 16-bit language id: two layouts of one language (French
//!   AZERTY and US under French) must stay apart, and nothing here loads or replaces a layout
//!   (decision 5).
//! - **Switching** posts `WM_INPUTLANGCHANGEREQUEST` to the foreground thread's focus window; never a
//!   blocking `SendMessage`.

// Device naming and the layout list serve the dev spike now, and the settings page next.
#![cfg_attr(not(feature = "dev"), allow(dead_code))]

#[cfg(feature = "dev")]
pub mod spike;
mod switcher;

pub use switcher::{cancel_capture, capture, take_captured, Switcher};

use core::ffi::c_void;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Parent, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROPTYPE, DEVPROP_TYPE_STRING};
use windows::Win32::Foundation::{DEVPROPKEY, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::{GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SLOCALIZEDDISPLAYNAME};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices, HRAWINPUT,
    RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDEV_REMOVE,
    RIDI_DEVICENAME, RID_HEADER, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, PostMessageW, GUITHREADINFO,
    WM_INPUTLANGCHANGEREQUEST,
};

use crate::win::wide;

/// A keyboard Raw Input knows about: its handle for this session, and its interface path, which is
/// stable across reconnects (a Bluetooth one carries the MAC) and is what the config stores.
#[derive(Clone, Debug)]
pub struct Keyboard {
    pub handle: usize,
    pub path: String,
}

/// Every keyboard interface Raw Input lists. One physical keyboard often has several.
pub fn keyboards() -> Vec<Keyboard> {
    let entry = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
    let mut count = 0u32;
    if unsafe { GetRawInputDeviceList(None, &mut count, entry) } == u32::MAX || count == 0 {
        return Vec::new();
    }
    let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
    let got = unsafe { GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, entry) };
    if got == u32::MAX {
        return Vec::new();
    }
    list.truncate(got as usize);
    list.iter()
        .filter(|device| device.dwType == RIM_TYPEKEYBOARD)
        .filter_map(|device| {
            let handle = device.hDevice.0 as usize;
            Some(Keyboard { handle, path: device_path(handle)? })
        })
        .collect()
}

/// A Raw Input device's interface path (`RIDI_DEVICENAME`).
pub fn device_path(handle: usize) -> Option<String> {
    let device = HANDLE(handle as *mut c_void);
    let mut chars = 0u32;
    unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &mut chars) };
    if chars == 0 {
        return None;
    }
    let mut buf = vec![0u16; chars as usize + 1];
    let got = unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, Some(buf.as_mut_ptr().cast()), &mut chars) };
    if got == u32::MAX || got == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..got as usize]);
    Some(path.trim_end_matches('\0').to_owned())
}

/// The PnP instance id behind an interface path:
/// `\\?\HID#VID_046D&PID_C539&MI_00#7&2b1f…&0&0000#{884b96c3-…}` → `HID\VID_046D&PID_C539&MI_00\7&2b1f…&0&0000`.
pub fn instance_id(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\").or_else(|| path.strip_prefix(r"\??\"))?;
    let (id, _class) = rest.rsplit_once("#{")?;
    Some(id.replace('#', "\\"))
}

/// The device's name, then its parents' (nearest first, at most `depth` in all): Windows names
/// nearly every keyboard "Périphérique clavier PIH", so the receiver or the Bluetooth device above
/// it is what tells them apart.
pub fn device_names(path: &str, depth: usize) -> Vec<String> {
    let Some(id) = instance_id(path) else {
        return Vec::new();
    };
    let id = wide(&id);
    let mut node = 0u32;
    if unsafe { CM_Locate_DevNodeW(&mut node, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) } != CR_SUCCESS {
        return Vec::new();
    }
    let friendly = devpkey(0xa45c254e_df1c_4efd_8020_67d146a850e0, 14);
    let description = devpkey(0xa45c254e_df1c_4efd_8020_67d146a850e0, 2);
    let mut names = Vec::new();
    for _ in 0..depth {
        if let Some(name) = unsafe { string_prop(node, &friendly) }.or_else(|| unsafe { string_prop(node, &description) }) {
            names.push(name);
        }
        let mut parent = 0u32;
        if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
            break;
        }
        node = parent;
    }
    names
}

/// Names Windows gives every keyboard or hub, which tell the user nothing.
fn generic(name: &str) -> bool {
    const WORDS: [&str; 12] =
        ["hid", "pih", "usb", "standard", "périphérique", "peripherique", "device", "appareil", "concentrateur", "hub", "composite", "filter"];
    let lower = name.to_lowercase();
    WORDS.iter().any(|word| lower.contains(word))
}

/// A name for a keyboard just added: its own or a parent's specific name (a Bluetooth keyboard's,
/// a receiver's), "Built-in keyboard" for the laptop's, else "Keyboard N" not yet `taken`.
pub fn default_name(path: &str, names: &[String], taken: &[String]) -> String {
    if let Some(name) = names.iter().find(|name| !generic(name)) {
        return name.clone();
    }
    if connection(path) == Connection::BuiltIn {
        return "Built-in keyboard".to_owned();
    }
    (1..).map(|n| format!("Keyboard {n}")).find(|name| !taken.contains(name)).unwrap_or_default()
}

/// How a keyboard is attached, read from its interface path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Connection {
    BuiltIn,
    Bluetooth,
    Usb,
    Other,
}

impl Connection {
    pub fn label(self) -> &'static str {
        match self {
            Connection::BuiltIn => "Built-in",
            Connection::Bluetooth => "Bluetooth",
            Connection::Usb => "USB",
            Connection::Other => "Keyboard",
        }
    }
}

pub fn connection(path: &str) -> Connection {
    let upper = path.to_uppercase();
    // Bluetooth LE HID (the HOGP service, 0x1812) or classic Bluetooth HID.
    if upper.contains("{00001812-") || upper.contains("BTHENUM") || upper.contains("BTHLE") {
        Connection::Bluetooth
    } else if upper.starts_with(r"\\?\ACPI#") {
        Connection::BuiltIn
    } else if upper.contains("VID_") {
        Connection::Usb
    } else {
        Connection::Other
    }
}

fn devpkey(fmtid: u128, pid: u32) -> DEVPROPKEY {
    DEVPROPKEY { fmtid: GUID::from_u128(fmtid), pid }
}

/// A string devnode property.
unsafe fn string_prop(node: u32, key: &DEVPROPKEY) -> Option<String> {
    let mut kind = DEVPROPTYPE(0);
    let mut size = 0u32;
    CM_Get_DevNode_PropertyW(node, key, &mut kind, None, &mut size, 0);
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if CM_Get_DevNode_PropertyW(node, key, &mut kind, Some(buf.as_mut_ptr()), &mut size, 0) != CR_SUCCESS
        || kind != DEVPROP_TYPE_STRING
    {
        return None;
    }
    let units: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
    let text = String::from_utf16_lossy(&units);
    let text = text.trim_end_matches('\0');
    (!text.is_empty()).then(|| text.to_owned())
}

/// Start or stop hearing which keyboard types, at `target` (keyboards: usage page 1, usage 6). In
/// the background too (`RIDEV_INPUTSINK`), with arrivals and removals (`RIDEV_DEVNOTIFY`). A
/// process has one registration per usage: a second one replaces the first.
pub fn register(target: HWND, on: bool) -> windows::core::Result<()> {
    let device = RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: 6,
        dwFlags: if on { RIDEV_INPUTSINK | RIDEV_DEVNOTIFY } else { RIDEV_REMOVE },
        hwndTarget: if on { target } else { HWND::default() },
    };
    unsafe { RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32) }
}

/// The keyboard behind a `WM_INPUT`, from its header alone: the key is never read. `Some(0)` is
/// input with no device (injected, e.g. `SendInput`).
pub fn source(lparam: LPARAM) -> Option<usize> {
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let mut header = RAWINPUTHEADER::default();
    let mut size = header_size;
    let got = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut c_void),
            RID_HEADER,
            Some((&mut header as *mut RAWINPUTHEADER).cast()),
            &mut size,
            header_size,
        )
    };
    (got != u32::MAX && got != 0).then_some(header.hDevice.0 as usize)
}

/// An installed layout, as `GetKeyboardLayoutList` gives it.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The full handle: what is stored and switched to.
    pub hkl: u64,
    /// The layout id (`KLID`, e.g. `00000409`) if it could be worked out.
    pub klid: Option<String>,
    /// "US", "Français", … in the display language.
    pub name: String,
    /// "Français (France)", … the language it is installed under.
    pub language: String,
}

impl std::fmt::Display for Layout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} — {}", self.name, self.language)
    }
}

/// A layout as the user reads it: its name, with its language only when another installed
/// layout has the same name ("US — Français" vs "US — English").
pub fn label(layout: &Layout, installed: &[Layout]) -> String {
    if installed.iter().any(|other| other.hkl != layout.hkl && other.name == layout.name) {
        layout.to_string()
    } else {
        layout.name.clone()
    }
}

/// The installed layouts, in Windows' order.
pub fn layouts() -> Vec<Layout> {
    let count = unsafe { GetKeyboardLayoutList(None) };
    if count <= 0 {
        return Vec::new();
    }
    let mut handles = vec![HKL::default(); count as usize];
    let got = unsafe { GetKeyboardLayoutList(Some(&mut handles)) }.max(0) as usize;
    handles[..got.min(handles.len())].iter().map(|hkl| describe(hkl.0 as usize as u64)).collect()
}

/// What a layout handle is, from the registry and the locale API.
pub fn describe(hkl: u64) -> Layout {
    let klid = klid(hkl);
    let name = klid.as_deref().and_then(layout_name).unwrap_or_else(|| format!("{hkl:08X}"));
    let language = language_name((hkl & 0xFFFF) as u16).unwrap_or_else(|| format!("{:04X}", hkl & 0xFFFF));
    Layout { hkl, klid, name, language }
}

/// The layout id behind a handle. The high word is the layout's device id: a plain one is the
/// `KLID` itself; `Fxxx` is a substitute (its low 12 bits are a `Layout Id` in the registry);
/// `Exxx` is an IME, whose `KLID` is the whole handle.
pub fn klid(hkl: u64) -> Option<String> {
    let device = ((hkl >> 16) & 0xFFFF) as u16;
    match device & 0xF000 {
        0xE000 => Some(format!("{:08X}", hkl as u32)),
        0xF000 => klid_by_layout_id(device & 0x0FFF),
        _ => Some(format!("{device:08X}")),
    }
}

const LAYOUTS_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts";

/// The `KLID` whose `Layout Id` value is `id`.
fn klid_by_layout_id(id: u16) -> Option<String> {
    let mut key = HKEY::default();
    let path = wide(LAYOUTS_KEY);
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()), Some(0), KEY_READ, &mut key) }.0 != 0 {
        return None;
    }
    let mut found = None;
    for index in 0.. {
        let mut name = [0u16; 64];
        let mut len = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(key, index, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None)
        };
        if status.0 != 0 {
            break;
        }
        let klid = String::from_utf16_lossy(&name[..len as usize]);
        let layout_id = hklm_string(&format!(r"{LAYOUTS_KEY}\{klid}"), "Layout Id");
        if layout_id.and_then(|v| u16::from_str_radix(v.trim(), 16).ok()) == Some(id) {
            found = Some(klid);
            break;
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    found
}

/// A layout's display name: `Layout Display Name` (localised, an indirect string), else
/// `Layout Text`.
fn layout_name(klid: &str) -> Option<String> {
    let key = format!(r"{LAYOUTS_KEY}\{klid}");
    let indirect = hklm_string(&key, "Layout Display Name").and_then(|source| {
        let source = wide(&source);
        let mut out = [0u16; 256];
        unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut out, None) }.ok()?;
        let len = out.iter().position(|&c| c == 0).unwrap_or(out.len());
        Some(String::from_utf16_lossy(&out[..len]))
    });
    indirect.filter(|name| !name.is_empty()).or_else(|| hklm_string(&key, "Layout Text"))
}

/// A language id's name in the display language ("Français (France)").
fn language_name(langid: u16) -> Option<String> {
    let mut locale = [0u16; 85];
    if unsafe { LCIDToLocaleName(u32::from(langid), Some(&mut locale), 0) } == 0 {
        return None;
    }
    let mut out = [0u16; 128];
    let len = unsafe { GetLocaleInfoEx(PCWSTR(locale.as_ptr()), LOCALE_SLOCALIZEDDISPLAYNAME, Some(&mut out)) };
    (len > 1).then(|| String::from_utf16_lossy(&out[..len as usize - 1]))
}

/// A `REG_SZ` under `HKEY_LOCAL_MACHINE`.
fn hklm_string(subkey: &str, name: &str) -> Option<String> {
    let subkey = wide(subkey);
    let name = wide(name);
    let mut buf = [0u16; 512];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.0 != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// The window keys go to, and the layout its thread has now.
#[derive(Clone, Copy, Debug)]
pub struct Foreground {
    /// The foreground thread's focus window, or the foreground window itself.
    pub target: HWND,
    pub window: HWND,
    pub pid: u32,
    /// [`Self::target`]'s thread.
    pub thread: u32,
    /// The layout of [`Self::target`]'s thread (not the foreground window's: see [`foreground`]).
    pub layout: u64,
}

/// The foreground window's thread, its focus window and its current layout.
pub fn foreground() -> Option<Foreground> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    let thread = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if thread == 0 {
        return None;
    }
    let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let focus = unsafe { GetGUIThreadInfo(thread, &mut info) }.ok().map(|_| info.hwndFocus).filter(|h| !h.is_invalid());
    let target = focus.unwrap_or(window);
    // The focus window's own thread, which can be another process's: Teams types into a WebView2
    // child, whose layout changes while the host window's thread keeps reporting the old one.
    let focus_thread = unsafe { GetWindowThreadProcessId(target, None) };
    let thread = if focus_thread != 0 { focus_thread } else { thread };
    let layout = unsafe { GetKeyboardLayout(thread) }.0 as usize as u64;
    Some(Foreground { target, window, pid, thread, layout })
}

/// Ask the foreground window to switch to `hkl` (posted, so a hung window cannot block us). An
/// elevated window ignores it (UIPI); the layout then follows on the next focus change.
pub fn switch(foreground: &Foreground, hkl: u64) -> bool {
    unsafe { PostMessageW(Some(foreground.target), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(hkl as isize)) }.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_path_maps_to_its_instance_id() {
        let path = r"\\?\HID#VID_046D&PID_C539&MI_00#7&2b1f3e9&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        assert_eq!(instance_id(path).as_deref(), Some(r"HID\VID_046D&PID_C539&MI_00\7&2b1f3e9&0&0000"));
        assert_eq!(instance_id("not a path"), None);
    }

    #[test]
    fn a_new_keyboard_gets_the_most_telling_name_there_is() {
        let usb = r"\\?\HID#VID_19F5&PID_2620&MI_01#9&eb2c70e&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        let laptop = r"\\?\ACPI#HPQ8002#4&102e92db&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // A Bluetooth keyboard's own name sits above the generic HID node.
        assert_eq!(default_name(usb, &names(&["Périphérique clavier PIH", "K380"]), &[]), "K380");
        // Nothing but Windows' generic names: numbered, skipping the taken ones.
        let generic = names(&["Périphérique clavier PIH", "Périphérique d’entrée USB", "Concentrateur USB générique"]);
        assert_eq!(default_name(usb, &generic, &names(&["Keyboard 1"])), "Keyboard 2");
        assert_eq!(default_name(laptop, &names(&["Standard 101/102-Key or Microsoft Natural PS/2 Keyboard"]), &[]), "Built-in keyboard");
        assert_eq!(connection(usb), Connection::Usb);
        assert_eq!(connection(laptop), Connection::BuiltIn);
        assert_eq!(connection(r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02046d_PID&b02e#a&1&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"), Connection::Bluetooth);
    }

    #[test]
    fn a_layout_handle_names_its_layout_not_its_language() {
        // US under French: the layout is US, whatever the language word says.
        assert_eq!(klid(0x0409_040C).as_deref(), Some("00000409"));
        assert_eq!(klid(0x040C_040C).as_deref(), Some("0000040C"));
        // An IME's KLID is the whole handle.
        assert_eq!(klid(0xE001_0411).as_deref(), Some("E0010411"));
    }
}
