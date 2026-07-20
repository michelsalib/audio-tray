//! Bluetooth battery levels via the PnP configuration manager.
//!
//! Battery isn't a property of the audio endpoint — it lives on a sibling device node
//! (typically "<name> Hands-Free AG") as `DEVPKEY_Bluetooth_Battery`. All functions of one
//! physical device share a `ContainerId`, so we enumerate every present device node, read
//! the battery + container of the ones that report a battery, and the caller matches by
//! the audio endpoint's own `ContainerId` (see `wasapi::describe`). Devices that don't
//! report battery to Windows (many do not) simply never appear here.

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
    CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_PRESENT, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROPTYPE, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID};
use windows::Win32::Foundation::DEVPROPKEY;

/// `(ContainerId `{GUID}` string, battery 0..=100)` for every present device that reports
/// a Bluetooth battery level. Best-effort — returns empty on any failure.
pub fn levels() -> Vec<(String, u8)> {
    unsafe { collect() }.unwrap_or_default()
}

unsafe fn collect() -> Option<Vec<(String, u8)>> {
    // The whole present-device instance-id list, as a double-NUL-terminated multi-string.
    let mut len = 0u32;
    if CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR::null(), CM_GETIDLIST_FILTER_PRESENT)
        != CR_SUCCESS
        || len == 0
    {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    if CM_Get_Device_ID_ListW(PCWSTR::null(), &mut buf, CM_GETIDLIST_FILTER_PRESENT) != CR_SUCCESS {
        return None;
    }

    let battery_key = devpkey(0x104E_A319_6EE2_4701_BD47_8DDB_F425_BBE5, 2);
    let container_key = devpkey(0x8C7E_D206_3F8A_4827_B3AB_AE9E_1FAE_FC6C, 2);

    let mut out = Vec::new();
    for id in buf.split(|&c| c == 0).filter(|s| !s.is_empty()) {
        let wide: Vec<u16> = id.iter().copied().chain(std::iter::once(0)).collect();
        let mut devinst = 0u32;
        if CM_Locate_DevNodeW(&mut devinst, PCWSTR(wide.as_ptr()), CM_LOCATE_DEVNODE_NORMAL)
            != CR_SUCCESS
        {
            continue;
        }
        let Some(pct) = byte_prop(devinst, &battery_key) else { continue };
        let Some(container) = guid_prop(devinst, &container_key) else { continue };
        out.push((guid_string(&container), pct.min(100)));
    }
    Some(out)
}

fn devpkey(fmtid: u128, pid: u32) -> DEVPROPKEY {
    DEVPROPKEY { fmtid: GUID::from_u128(fmtid), pid }
}

/// Read a single-byte devnode property (e.g. the battery percentage).
unsafe fn byte_prop(devinst: u32, key: &DEVPROPKEY) -> Option<u8> {
    let mut ptype = DEVPROPTYPE(0);
    let mut val = 0u8;
    let mut size = 1u32;
    let ret = CM_Get_DevNode_PropertyW(devinst, key, &mut ptype, Some(&mut val), &mut size, 0);
    (ret == CR_SUCCESS && ptype == DEVPROP_TYPE_BYTE).then_some(val)
}

/// Read a GUID devnode property (e.g. the container id).
unsafe fn guid_prop(devinst: u32, key: &DEVPROPKEY) -> Option<GUID> {
    let mut ptype = DEVPROPTYPE(0);
    let mut g = GUID::from_u128(0);
    let mut size = std::mem::size_of::<GUID>() as u32;
    let ret = CM_Get_DevNode_PropertyW(
        devinst,
        key,
        &mut ptype,
        Some(&mut g as *mut GUID as *mut u8),
        &mut size,
        0,
    );
    (ret == CR_SUCCESS && ptype == DEVPROP_TYPE_GUID).then_some(g)
}

/// Format a GUID as an upper-case `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` string, matching
/// what `PropVariantToStringAlloc` yields for the endpoint's own container id.
fn guid_string(g: &GUID) -> String {
    let d = g.data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}
