//! What the receiver sources ([`super::hidpp`], [`super::razer`]) share: finding a vendor's HID
//! collections, telling their usages, and watching for a receiver plugged in or out.

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW, CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps, HIDP_CAPS, HIDP_STATUS_SUCCESS,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// A present HID collection of the vendor's.
pub(super) struct Collection {
    /// Its interface path, NUL-terminated.
    pub path: Vec<u16>,
    pub caps: HIDP_CAPS,
}

impl Collection {
    pub fn is(&self, usage_page: u16, usage: u16) -> bool {
        self.caps.UsagePage == usage_page && self.caps.Usage == usage
    }

    /// The path, as text.
    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.path[..self.path.len() - 1])
    }

    /// The interface it belongs to: its path without the collection number, so every collection
    /// of one receiver shares it. See [`interface_key`].
    pub fn interface(&self) -> String {
        interface_key(&self.text())
    }
}

/// Every present HID collection of the vendor's (USB: `vid_xxxx`; Bluetooth ones, `vid&02xxxx`,
/// are left to the Bluetooth source).
pub(super) fn collections(vendor: u16) -> Vec<Collection> {
    let tag = format!("vid_{vendor:04x}");
    unsafe { paths() }
        .into_iter()
        .filter(|path| String::from_utf16_lossy(path).to_ascii_lowercase().contains(&tag))
        .filter_map(|path| Some(Collection { caps: unsafe { caps(&path) }?, path }))
        .collect()
}

/// The device-watcher filter for the vendor's HID interfaces: a receiver plugged in or out.
pub(super) fn watch_filter(vendor: u16) -> String {
    format!(r#"System.Devices.InterfaceClassGuid:="{{4D1E55B2-F16F-11CF-88CB-001111000030}}" AND System.DeviceInterface.Hid.VendorId:={vendor}"#)
}

/// `\\?\hid#vid_046d&pid_c539&mi_02&col01#9&37a0ad40&0&0000#{4d1e…}` gives
/// `VID_046D&PID_C539&MI_02#9&37A0AD40&0`.
fn interface_key(path: &str) -> String {
    let mut parts = path.split('#').skip(1);
    let hardware: Vec<&str> = parts.next().unwrap_or_default().split('&').filter(|p| !p.to_ascii_lowercase().starts_with("col")).collect();
    let instance = parts.next().unwrap_or_default();
    let instance = instance.rsplit_once('&').map_or(instance, |(head, _)| head);
    format!("{}#{}", hardware.join("&"), instance).to_ascii_uppercase()
}

/// A file handle, closed on drop.
pub(super) struct Handle(pub HANDLE);

impl Handle {
    /// Open a collection. `access` 0 is enough for its usages and feature reports, and is allowed
    /// even on one Windows holds (a mouse's own).
    pub unsafe fn open(path: &[u16], access: u32, flags: FILE_FLAGS_AND_ATTRIBUTES) -> Option<Self> {
        CreateFileW(PCWSTR(path.as_ptr()), access, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, flags, None).ok().map(Handle)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Every present HID interface's path, NUL-terminated.
unsafe fn paths() -> Vec<Vec<u16>> {
    let guid = HidD_GetHidGuid();
    // A device arriving between the two calls makes the second fail: try again once.
    for _ in 0..2 {
        let mut len = 0u32;
        if CM_Get_Device_Interface_List_SizeW(&mut len, &guid, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS || len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_Interface_ListW(&guid, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT) == CR_SUCCESS {
            return buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| s.iter().copied().chain(std::iter::once(0)).collect()).collect();
        }
    }
    Vec::new()
}

/// A collection's usage and report lengths.
unsafe fn caps(path: &[u16]) -> Option<HIDP_CAPS> {
    let handle = Handle::open(path, 0, FILE_FLAGS_AND_ATTRIBUTES(0))?;
    let mut data = PHIDP_PREPARSED_DATA::default();
    if !HidD_GetPreparsedData(handle.0, &mut data) {
        return None;
    }
    let mut caps = HIDP_CAPS::default();
    let ok = HidP_GetCaps(data, &mut caps) == HIDP_STATUS_SUCCESS;
    let _ = HidD_FreePreparsedData(data);
    ok.then_some(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_collection_of_an_interface_shares_its_key() {
        let short = r"\\?\hid#vid_046d&pid_c539&mi_02&col01#9&37a0ad40&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let long = r"\\?\hid#vid_046d&pid_c539&mi_02&col02#9&37a0ad40&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(interface_key(short), "VID_046D&PID_C539&MI_02#9&37A0AD40&0");
        assert_eq!(interface_key(short), interface_key(long));
    }

    #[test]
    fn the_watch_filter_names_the_vendor_in_decimal() {
        assert!(watch_filter(0x046D).ends_with("VendorId:=1133"));
    }
}
