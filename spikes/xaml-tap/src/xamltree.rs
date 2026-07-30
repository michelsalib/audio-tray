//! Walking the live XAML tree via `VisualTreeHelper` (M2).
//!
//! `IVisualTreeService`'s own property/collection API doesn't work in Explorer
//! (see [`crate::walk`]), so enumeration goes through the real WinRT XAML API
//! instead: turn a diagnostics handle into an `IInspectable`, cast it to
//! `IDependencyObject`, then use `VisualTreeHelper` to climb and descend.
//!
//! Must run on the XAML UI thread.

// Retained as a regression probe: these routes are non-functional in Explorer today
// (see FINDINGS.md), so nothing calls them, but re-running them is how we'd notice
// a future Windows build changing that.
#![allow(dead_code)]

use crate::log::logf;
use crate::winrt::{IDependencyObject, IVisualTreeHelperStatics, VISUAL_TREE_HELPER};
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};
use core::ffi::c_void;
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows_core::{IInspectable, Interface, HSTRING};

/// Caps on the walk, so a surprise in the tree can't hang Explorer's UI thread.
const MAX_ELEMENTS: u32 = 6000;
const MAX_DEPTH: usize = 60;

/// The types M2 is hunting for.
const INTERESTING: &[&str] = &[
    "SystemTray.IconView",
    "SystemTray.OmniButton",
    "SystemTray.NotifyIconView",
    "Taskbar.TaskListButton",
];

pub struct Walker {
    statics: IVisualTreeHelperStatics,
    elements: u32,
    found: Vec<(String, usize)>,
}

/// Walks every tree we can find a way into, and logs them.
///
/// # Safety
/// Must be called on the XAML UI thread with a live diagnostics site.
pub unsafe fn dump_tree(diagnostics: &IXamlDiagnostics, feed_roots: &[InstanceHandle]) {
    let Some(statics) = visual_tree_helper() else {
        return;
    };

    // The UI layer is a real element of the visible tree, so it is a far better
    // way in than the popup roots, which are empty until something pops up.
    let mut seeds: Vec<(String, InstanceHandle)> = Vec::new();
    let mut layer: *mut c_void = core::ptr::null_mut();
    if diagnostics.GetUiLayer(&mut layer) == S_OK && !layer.is_null() {
        let owned = core::mem::transmute::<*mut c_void, windows_core::IUnknown>(layer);
        let mut handle: InstanceHandle = 0;
        if diagnostics.GetHandleFromIInspectable(owned.as_raw(), &mut handle) == S_OK {
            seeds.push(("UiLayer".into(), handle));
        }
    }
    for (n, &root) in feed_roots.iter().enumerate() {
        seeds.push((format!("feedRoot{n}"), root));
    }

    let mut walked_roots: Vec<*mut c_void> = Vec::new();
    for (label, seed) in seeds {
        let Some(root) = climb_to_root(&statics, diagnostics, seed) else {
            logf!("seed {label} (0x{seed:x}): could not resolve");
            continue;
        };
        // Several seeds usually share one root; only dump it once.
        if walked_roots.contains(&root.as_raw()) {
            logf!("seed {label}: same root as an earlier seed, skipping");
            continue;
        }
        walked_roots.push(root.as_raw());

        let mut walker = Walker {
            statics: statics.clone(),
            elements: 0,
            found: Vec::new(),
        };
        logf!("===== XAML tree from {label} =====");
        walker.visit(&root, 0);
        logf!("===== {} elements from {label} =====", walker.elements);
        for (name, depth) in &walker.found {
            logf!("MATCH depth={depth} {name}");
        }
    }
}

fn visual_tree_helper() -> Option<IVisualTreeHelperStatics> {
    match unsafe { RoGetActivationFactory(&HSTRING::from(VISUAL_TREE_HELPER)) } {
        Ok(statics) => Some(statics),
        Err(err) => {
            logf!("RoGetActivationFactory({VISUAL_TREE_HELPER}) failed: {err}");
            None
        }
    }
}

/// Walks the tree that `seed` belongs to.
///
/// The seed has to be an element that is genuinely part of the tree of interest:
/// `GetUiLayer` and the popup roots all sit in an empty XAML core in Explorer, so
/// the only way into the taskbar's core is a handle harvested from the live feed.
///
/// # Safety
/// Must be called on the XAML UI thread with a live diagnostics site.
#[allow(dead_code)] // kept: the seed-based walk is the fallback if the v2 replay ever regresses
pub unsafe fn dump_seed(diagnostics: &IXamlDiagnostics, seed: InstanceHandle, label: &str) {
    let Some(statics) = visual_tree_helper() else {
        return;
    };
    let Some(root) = climb_to_root(&statics, diagnostics, seed) else {
        logf!("seed {label} (0x{seed:x}): could not resolve");
        return;
    };
    let mut walker = Walker {
        statics,
        elements: 0,
        found: Vec::new(),
    };
    logf!("===== XAML tree from {label} =====");
    walker.visit(&root, 0);
    logf!("===== {} elements from {label} =====", walker.elements);
    for (name, depth) in &walker.found {
        logf!("MATCH depth={depth} {name}");
    }
}

/// Resolves a diagnostics handle and climbs to the top of its tree.
unsafe fn climb_to_root(
    statics: &IVisualTreeHelperStatics,
    diagnostics: &IXamlDiagnostics,
    seed: InstanceHandle,
) -> Option<IDependencyObject> {
    let object = object_from_handle(diagnostics, seed)?;
    let mut node = object.cast::<IDependencyObject>().ok()?;
    let mut climbed = 0;
    loop {
        let mut parent: *mut c_void = core::ptr::null_mut();
        let hr = statics.GetParent(node.as_raw(), &mut parent);
        if hr != S_OK {
            logf!("seed 0x{seed:x}: GetParent -> 0x{:08x}", hr.0);
            break;
        }
        if parent.is_null() {
            break;
        }
        node = core::mem::transmute::<*mut c_void, IDependencyObject>(parent);
        climbed += 1;
        if climbed > MAX_DEPTH {
            break;
        }
    }
    logf!("seed 0x{seed:x}: climbed {climbed} levels to its root");
    Some(node)
}

impl Walker {
    unsafe fn visit(&mut self, node: &IDependencyObject, depth: usize) {
        if self.elements >= MAX_ELEMENTS || depth > MAX_DEPTH {
            return;
        }
        self.elements += 1;

        let class = node
            .cast::<IInspectable>()
            .ok()
            .and_then(|object| object.GetRuntimeClassName().ok())
            .map(|name| name.to_string())
            .unwrap_or_else(|| "<unknown>".into());
        logf!("{}{class}", "  ".repeat(depth));

        if INTERESTING.iter().any(|target| class.contains(target)) {
            self.found.push((class, depth));
        }

        let mut count = 0i32;
        let hr = self.statics.GetChildrenCount(node.as_raw(), &mut count);
        if hr != S_OK {
            logf!("{}  GetChildrenCount -> 0x{:08x}", "  ".repeat(depth), hr.0);
            return;
        }
        if count <= 0 {
            return;
        }
        for index in 0..count {
            let mut child: *mut c_void = core::ptr::null_mut();
            if self.statics.GetChild(node.as_raw(), index, &mut child) != S_OK || child.is_null() {
                continue;
            }
            let child = core::mem::transmute::<*mut c_void, IDependencyObject>(child);
            self.visit(&child, depth + 1);
        }
    }
}

unsafe fn object_from_handle(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<IInspectable> {
    let mut raw: *mut c_void = core::ptr::null_mut();
    if diagnostics.GetIInspectableFromHandle(handle, &mut raw) != S_OK || raw.is_null() {
        return None;
    }
    Some(core::mem::transmute::<*mut c_void, IInspectable>(raw))
}
