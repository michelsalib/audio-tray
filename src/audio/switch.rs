//! Isolated wrapper around the undocumented `IPolicyConfig` (via `com-policy-config`).
//!
//! This is the ONLY module that imports `com-policy-config` (plan §9). The crate is
//! solo-maintained and carries a "may contain port mistakes" warning, so the blast
//! radius is confined here: if it breaks, the raw `IPolicyConfig` COM binding can be
//! inlined in this file without touching any other module.

use anyhow::{Context, Result};
use com_policy_config::{IPolicyConfig, PolicyConfigClient};
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use super::DeviceId;

/// Set the default render endpoint across ALL THREE roles. Setting only one role is a
/// common bug that leaves some apps (notably communications apps) on the old device
/// (plan §2.3). COM must already be initialized on the calling thread.
pub fn set_default(id: &DeviceId) -> Result<()> {
    // NUL-terminated UTF-16 buffer; must outlive every SetDefaultEndpoint call below.
    let wide: Vec<u16> = id.0.encode_utf16().chain(std::iter::once(0)).collect();
    let name = PCWSTR(wide.as_ptr());

    unsafe {
        let policy: IPolicyConfig = CoCreateInstance(&PolicyConfigClient, None, CLSCTX_ALL)
            .context("create PolicyConfigClient (IPolicyConfig)")?;
        for role in [eConsole, eMultimedia, eCommunications] {
            policy
                .SetDefaultEndpoint(name, role)
                .with_context(|| format!("SetDefaultEndpoint(role={})", role.0))?;
        }
    }
    Ok(())
}
