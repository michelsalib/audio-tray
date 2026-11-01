//! Teams' meeting window, through UI Automation: the mute for an organisation whose admin turned
//! Teams' local API off (the switch is then missing from Teams' settings). Screen readers rely on
//! the same tree, so no policy removes it.
//!
//! The meeting's mic button has a fixed `AutomationId` (`microphone-button`) whatever the language;
//! its name says the state ("Couper le micro" while heard, "Réactiver le micro" while muted). It
//! has no toggle state, so the name is read, and followed through a property-changed event: no
//! polling. Pressing it is its Invoke pattern, as a screen reader would.
//!
//! One thread (UI Automation wants an MTA, and its calls block), asleep on a channel: woken when
//! Teams takes or releases the microphone (a meeting starting or ending), and for a mute.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use windows::core::{implement, Interface, BSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VariantClear, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BSTR};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationPropertyChangedEventHandler, IUIAutomationPropertyChangedEventHandler_Impl, TreeScope_Descendants,
    TreeScope_Element, UIA_AutomationIdPropertyId, UIA_InvokePatternId, UIA_NamePropertyId, UIA_PROPERTY_ID,
};

use crate::hub::{Call, Event, VoiceApp};

/// The integration's name in the hub, for Settings' status.
pub const NAME: &str = "Teams meeting";
const MIC_BUTTON: &str = "microphone-button";
const WINDOW_CLASS: &str = "TeamsWebView";

enum Command {
    /// Teams holds the microphone (`true`: a meeting is on) or let it go.
    Meeting(bool),
    /// Mute (`Some(true)`), unmute, or toggle (`None`).
    Mute(Option<bool>),
    /// The mic button's name changed.
    Changed,
}

static COMMANDS: OnceLock<Sender<Command>> = OnceLock::new();
static ATTACHED: AtomicBool = AtomicBool::new(false);

fn send(command: Command) {
    let sender = COMMANDS.get_or_init(|| {
        let (sender, commands) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("teams-ui".into()).spawn(move || run(commands));
        sender
    });
    let _ = sender.send(command);
}

/// Follow the meeting window while Teams holds the microphone (`in_meeting`), let it go after.
pub fn meeting(in_meeting: bool) {
    if in_meeting || ATTACHED.load(Ordering::SeqCst) {
        send(Command::Meeting(in_meeting));
    }
}

/// Whether a meeting window's mic button is followed (so mutes can go to it).
pub fn attached() -> bool {
    ATTACHED.load(Ordering::SeqCst)
}

/// Mute (`Some(true)`), unmute, or toggle (`None`) through the meeting window.
pub fn set_mute(mute: Option<bool>) {
    send(Command::Mute(mute));
}

/// Whether the mic button's name means muted. Its id is fixed, its name localised: the names
/// asking to unmute, in English, French and German.
fn muted_from(name: &str) -> bool {
    let name = name.to_lowercase();
    ["unmute", "réactiver", "rétablir", "aufheben"].iter().any(|word| name.contains(word))
}

/// The meeting's name, from the window title (`"Réunion avec … | Microsoft Teams"`).
fn meeting_name(title: &str) -> Option<String> {
    let (name, _) = title.split_once(" | ")?;
    Some(name.trim().to_string()).filter(|name| !name.is_empty())
}

#[implement(IUIAutomationPropertyChangedEventHandler)]
struct NameChanged(Sender<Command>);

#[allow(non_snake_case)]
impl IUIAutomationPropertyChangedEventHandler_Impl for NameChanged_Impl {
    fn HandlePropertyChangedEvent(
        &self,
        _sender: windows::core::Ref<'_, IUIAutomationElement>,
        _property: UIA_PROPERTY_ID,
        _value: &VARIANT,
    ) -> windows::core::Result<()> {
        // Read on our own thread: a UIA call from inside its event is best avoided.
        let _ = self.0.send(Command::Changed);
        Ok(())
    }
}

/// The meeting window followed: its mic button and the handler on it.
struct Attached {
    button: IUIAutomationElement,
    handler: IUIAutomationPropertyChangedEventHandler,
    channel: Option<String>,
    muted: bool,
}

fn run(commands: Receiver<Command>) {
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        eprintln!("teams: UI Automation unavailable (COM)");
        return;
    }
    let automation: IUIAutomation = match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
        Ok(automation) => automation,
        Err(e) => {
            eprintln!("teams: UI Automation unavailable ({e})");
            return;
        }
    };
    let Some(sender) = COMMANDS.get().cloned() else { return };
    let mut attached: Option<Attached> = None;
    while let Ok(command) = commands.recv() {
        match command {
            Command::Meeting(true) if attached.is_none() => match attach(&automation, &sender) {
                Ok(found) => {
                    println!("teams: following the meeting window's mic button ({})", if found.muted { "muted" } else { "live" });
                    ATTACHED.store(true, Ordering::SeqCst);
                    crate::hub::emit(Event::Integration { name: NAME, connected: true });
                    emit(&found);
                    attached = Some(found);
                }
                Err(e) => println!("teams: no meeting window to follow ({e:#})"),
            },
            Command::Meeting(false) => {
                if let Some(old) = attached.take() {
                    detach(&automation, old);
                }
            }
            Command::Meeting(true) => {}
            Command::Changed => {
                let Some(found) = attached.as_mut() else { continue };
                match unsafe { found.button.CurrentName() } {
                    Ok(name) => {
                        let muted = muted_from(&name.to_string());
                        if muted != found.muted {
                            found.muted = muted;
                            emit(found);
                        }
                    }
                    // The window closed under us: the meeting is over.
                    Err(_) => {
                        if let Some(old) = attached.take() {
                            detach(&automation, old);
                        }
                    }
                }
            }
            Command::Mute(mute) => {
                let Some(found) = attached.as_ref() else { continue };
                if mute.is_some_and(|m| m == found.muted) {
                    continue;
                }
                let pressed = unsafe { found.button.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) }
                    .and_then(|invoke| unsafe { invoke.Invoke() });
                if let Err(e) = pressed {
                    eprintln!("teams: could not press the meeting's mic button ({e})");
                }
            }
        }
    }
}

fn emit(found: &Attached) {
    let call = Call { channel: found.channel.clone(), self_muted: found.muted, ..Default::default() };
    crate::hub::emit(Event::Call { app: VoiceApp::Teams, call: Some(call) });
}

fn detach(automation: &IUIAutomation, old: Attached) {
    let _ = unsafe { automation.RemovePropertyChangedEventHandler(&old.button, &old.handler) };
    ATTACHED.store(false, Ordering::SeqCst);
    crate::hub::emit(Event::Call { app: VoiceApp::Teams, call: None });
    crate::hub::emit(Event::Integration { name: NAME, connected: false });
    println!("teams: the meeting ended");
}

/// Find the meeting window's mic button and follow its name. Chromium builds its accessibility
/// tree on first demand, so an empty first look is retried a few times.
fn attach(automation: &IUIAutomation, sender: &Sender<Command>) -> Result<Attached> {
    let windows = teams_windows();
    anyhow::ensure!(!windows.is_empty(), "no Teams window");
    let id = BSTR::from(MIC_BUTTON);
    let mut value = VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { bstrVal: std::mem::ManuallyDrop::new(id) },
            }),
        },
    };
    let condition = unsafe { automation.CreatePropertyCondition(UIA_AutomationIdPropertyId, &value) };
    let _ = unsafe { VariantClear(&mut value) };
    let condition = condition.context("CreatePropertyCondition")?;
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(700));
        }
        for (hwnd, title) in &windows {
            let Ok(window) = (unsafe { automation.ElementFromHandle(*hwnd) }) else { continue };
            let Ok(button) = (unsafe { window.FindFirst(TreeScope_Descendants, &condition) }) else { continue };
            if button.as_raw().is_null() {
                continue;
            }
            let name = unsafe { button.CurrentName() }.map(|n| n.to_string()).unwrap_or_default();
            let handler: IUIAutomationPropertyChangedEventHandler = NameChanged(sender.clone()).into();
            unsafe { automation.AddPropertyChangedEventHandlerNativeArray(&button, TreeScope_Element, None, &handler, &[UIA_NamePropertyId]) }
                .context("AddPropertyChangedEventHandler")?;
            return Ok(Attached { button, handler, channel: meeting_name(title), muted: muted_from(&name) });
        }
    }
    anyhow::bail!("no mic button in {} Teams window(s)", windows.len())
}

/// Teams' top-level web views, with their titles.
fn teams_windows() -> Vec<(HWND, String)> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;

    let mut found = Vec::new();
    crate::win::enum_windows(|hwnd| {
        if crate::win::class_name(hwnd) == WINDOW_CLASS && crate::music::player::window_process(hwnd).as_deref() == Some("ms-teams.exe") {
            let mut buf = [0u16; 256];
            let len = unsafe { GetWindowTextW(hwnd, &mut buf) }.max(0) as usize;
            found.push((hwnd, String::from_utf16_lossy(&buf[..len])));
        }
        true
    });
    found
}

#[cfg(test)]
mod tests {
    use super::{meeting_name, muted_from};

    #[test]
    fn the_mic_buttons_name_says_whether_muted() {
        assert!(!muted_from("Couper le micro"));
        assert!(muted_from("Réactiver le micro"));
        assert!(!muted_from("Mute mic"));
        assert!(muted_from("Unmute mic"));
    }

    #[test]
    fn the_meeting_is_named_after_the_window() {
        assert_eq!(meeting_name("Réunion avec Michel SALIB | Microsoft Teams").as_deref(), Some("Réunion avec Michel SALIB"));
    }
}
