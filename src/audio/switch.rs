//! Isolated wrapper around the undocumented `IPolicyConfig` (via `com-policy-config`).
//!
//! The only module that imports `com-policy-config`, so the binding can be inlined here if it breaks.

use anyhow::{Context, Result};
use com_policy_config::{IPolicyConfig, PolicyConfigClient};
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use super::DeviceId;

/// Set the default endpoint for all three roles (one role leaves some apps behind).
/// COM must be initialized on the calling thread.
pub fn set_default(id: &DeviceId) -> Result<()> {
    // Bound to a local: the buffer must outlive every SetDefaultEndpoint call below.
    let wide = crate::win::wide(&id.0);
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
