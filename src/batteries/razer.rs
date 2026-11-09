//! Razer headsets on their USB receiver. Only the BlackShark V2 Pro (2020, `1532:0528`) for now:
//! the protocol is the one HaloBattery reads it with (`providers/blackshark2020.py`), taken from a
//! Synapse capture.
//!
//! Its vendor collection (usage page `FF00`, usage 1, 64-byte reports) is asked by writing feature
//! report `FF` and reading the same report back about 100 ms later:
//!
//! ```text
//! ask     FF 0A 00 FD 04 12 F1 02 05 00…
//! answer  FF 0F 05 FE 12 04 1F 08 05 ?? ?? <state> <mV, big-endian> <level %>
//! stale   FF 01 00 …   (not answered yet, or the headset is off: the receiver's last frame)
//! ```
//!
//! The level is coarse (100, 80, 50, 30, 10) and, while charging (state `09`), reads high.
//! Synapse talks to the receiver through its own driver: nothing here clashes with it.

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_Interface_PropertyW, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::HumanInterfaceDevice::{HidD_GetFeature, HidD_SetFeature};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_ContainerId, DEVPKEY_Device_InstanceId, DEVPROPTYPE};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;

use super::hid::{self, Collection, Handle};
use crate::hub::{Battery, BatteryKind};

pub(super) const VENDOR: u16 = 0x1532;
/// The receivers this protocol is known for, by product id, and the headset's name.
const MODELS: [(u16, &str); 1] = [(0x0528, "Razer BlackShark V2 Pro")];

const VENDOR_PAGE: u16 = 0xFF00;
const USAGE: u16 = 1;
const REPORT_LEN: usize = 64;
const ASK: [u8; 9] = [0xFF, 0x0A, 0x00, 0xFD, 0x04, 0x12, 0xF1, 0x02, 0x05];
const ANSWER: [u8; 9] = [0xFF, 0x0F, 0x05, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05];
/// Synapse waits this long before reading the answer; read up to [`TRIES`] times while stale.
const WAIT: std::time::Duration = std::time::Duration::from_millis(100);
const TRIES: usize = 5;

/// Every headset that is on, on a receiver plugged in; `None` when no receiver is plugged in.
pub(super) fn read() -> Option<Vec<Battery>> {
    let mut found = false;
    let mut list = Vec::new();
    for collection in hid::collections(VENDOR) {
        let Some(name) = model(&collection) else { continue };
        if !collection.is(VENDOR_PAGE, USAGE) || collection.caps.FeatureReportByteLength as usize != REPORT_LEN {
            continue;
        }
        found = true;
        let Some(level) = (unsafe { ask(&collection.path) }) else { continue };
        let id = unsafe { container(&collection.path) }.unwrap_or_else(|| format!("RAZER\\{}", collection.interface()));
        list.push(Battery { id, name: name.to_string(), kind: BatteryKind::Headset, level });
    }
    found.then_some(list)
}

/// The headset's name, when the collection is a receiver of a known model's.
fn model(collection: &Collection) -> Option<&'static str> {
    let path = collection.text().to_ascii_lowercase();
    MODELS.iter().find(|(pid, _)| path.contains(&format!("pid_{pid:04x}"))).map(|(_, name)| *name)
}

/// Ask the receiver: the level, or `None` when the headset is off (or never answered).
unsafe fn ask(path: &[u16]) -> Option<u8> {
    let handle = Handle::open(path, 0, FILE_FLAGS_AND_ATTRIBUTES(0))?;
    let mut request = [0u8; REPORT_LEN];
    request[..ASK.len()].copy_from_slice(&ASK);
    if !HidD_SetFeature(handle.0, request.as_ptr().cast(), REPORT_LEN as u32) {
        return None;
    }
    for _ in 0..TRIES {
        std::thread::sleep(WAIT);
        let mut reply = [0u8; REPORT_LEN];
        reply[0] = ASK[0];
        if !HidD_GetFeature(handle.0, reply.as_mut_ptr().cast(), REPORT_LEN as u32) {
            return None;
        }
        if let Some(level) = parse(&reply) {
            return Some(level);
        }
    }
    None
}

/// The level an answer gives; `None` for a stale frame or a headset with no voltage (off).
fn parse(reply: &[u8]) -> Option<u8> {
    if reply.len() < 15 || !reply.starts_with(&ANSWER) {
        return None;
    }
    let millivolts = u16::from_be_bytes([reply[12], reply[13]]);
    (millivolts != 0).then_some(reply[14].min(100))
}

/// The device container the collection belongs to (`{GUID}`): the headset's audio endpoints
/// share it, so its level also shows on its device row.
unsafe fn container(path: &[u16]) -> Option<String> {
    let mut ptype = DEVPROPTYPE(0);
    let mut instance = [0u16; 512];
    let mut size = std::mem::size_of_val(&instance) as u32;
    let ret = CM_Get_Device_Interface_PropertyW(PCWSTR(path.as_ptr()), &DEVPKEY_Device_InstanceId, &mut ptype, Some(instance.as_mut_ptr().cast()), &mut size, 0);
    if ret != CR_SUCCESS {
        return None;
    }
    let mut devinst = 0u32;
    if CM_Locate_DevNodeW(&mut devinst, PCWSTR(instance.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
        return None;
    }
    super::guid_prop(devinst, &DEVPKEY_Device_ContainerId).map(|g| super::guid_string(&g))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bytes: &[u8]) -> [u8; REPORT_LEN] {
        let mut f = [0u8; REPORT_LEN];
        f[..bytes.len()].copy_from_slice(bytes);
        f
    }

    /// Frames HaloBattery recorded from the headset, and the one the user's receiver held.
    #[test]
    fn answers_give_the_level_and_stale_frames_none() {
        let on_battery = frame(&[0xFF, 0x0F, 0x05, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05, 0x03, 0x05, 0x01, 0x0E, 0xC0, 0x50]);
        assert_eq!(parse(&on_battery), Some(80));
        let low = frame(&[0xFF, 0x0F, 0x05, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05, 0x03, 0x05, 0x03, 0x0D, 0xA0, 0x0A]);
        assert_eq!(parse(&low), Some(10));
        let off = frame(&[0xFF, 0x01, 0x00, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05, 0x03, 0x05, 0x01, 0x0E, 0xC0, 0x50]);
        assert_eq!(parse(&off), None);
        let idle = frame(&[0xFF, 0x01, 0x00, 0xFE, 0x00, 0x04, 0x0F, 0x05, 0x81, 0xAA, 0xC6, 0x01, 0x4E, 0x32, 0x31]);
        assert_eq!(parse(&idle), None);
    }
}
