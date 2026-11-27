//! Accumulates `OnVisualTreeChange` events into a tree and dumps it.
//!
//! `AdviseVisualTreeChange` replays the entire existing tree as a burst of `Add`
//! mutations before switching to live deltas, so "the burst went quiet" is the
//! signal that a complete snapshot has arrived. A watchdog thread waits for that
//! quiet period and writes an indented dump; if mutations resume it re-arms and
//! dumps again, which is how you watch a flyout open or the taskbar re-theme.

use crate::log::logf;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the event stream must be silent before we consider a dump worthwhile.
const QUIET_PERIOD: Duration = Duration::from_millis(1500);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

struct Node {
    parent: u64,
    child_index: u32,
    type_name: String,
    name: String,
    /// Insertion order, the tie-breaker when several children claim one index.
    seq: u64,
}

#[derive(Default)]
struct Tree {
    nodes: HashMap<u64, Node>,
    /// Indexes over `nodes`, kept in step by [`Tree::link`]/[`Tree::unlink`], so the sweep's lookups
    /// do not scan every recorded element.
    children: HashMap<u64, HashSet<u64>>,
    by_type: HashMap<String, HashSet<u64>>,
    by_name: HashMap<String, HashSet<u64>>,
    adds: u64,
    removes: u64,
    seq: u64,
    last_event: Option<Instant>,
    dirty: bool,
    dumps: u32,
}

impl Tree {
    fn link(&mut self, handle: u64, node: Node) {
        if let Some(old) = self.nodes.remove(&handle) {
            self.unindex(handle, &old);
        }
        self.children.entry(node.parent).or_default().insert(handle);
        self.by_type.entry(node.type_name.clone()).or_default().insert(handle);
        if !node.name.is_empty() {
            self.by_name.entry(node.name.clone()).or_default().insert(handle);
        }
        self.nodes.insert(handle, node);
    }

    fn unlink(&mut self, handle: u64) {
        if let Some(old) = self.nodes.remove(&handle) {
            self.unindex(handle, &old);
        }
    }

    fn unindex(&mut self, handle: u64, node: &Node) {
        fn drop_from(map: &mut HashMap<String, HashSet<u64>>, key: &str, handle: u64) {
            if let Some(list) = map.get_mut(key) {
                list.remove(&handle);
                if list.is_empty() {
                    map.remove(key);
                }
            }
        }
        if let Some(list) = self.children.get_mut(&node.parent) {
            list.remove(&handle);
            if list.is_empty() {
                self.children.remove(&node.parent);
            }
        }
        drop_from(&mut self.by_type, &node.type_name, handle);
        drop_from(&mut self.by_name, &node.name, handle);
    }
}

fn tree() -> &'static Mutex<Tree> {
    static TREE: OnceLock<Mutex<Tree>> = OnceLock::new();
    TREE.get_or_init(|| Mutex::new(Tree::default()))
}

fn lock() -> std::sync::MutexGuard<'static, Tree> {
    crate::lock(tree())
}

/// How many mutations get logged verbatim, for diagnosing what XAML sends us.
///
/// Observed on Win11 26200: for root elements XAML leaves the whole
/// `ParentChildRelation` zeroed and only `element.Handle` identifies the node,
/// so the element handle — not `relation.Child` — is the identity we key on.
const RAW_LOG_LIMIT: u64 = 60;

/// Records one mutation. Called on the XAML UI thread, so it stays cheap: a map
/// write and nothing else. All formatting happens on the watchdog thread.
#[allow(clippy::too_many_arguments)]
pub fn record(
    parent: u64,
    child: u64,
    child_index: u32,
    type_name: String,
    name: String,
    added: bool,
    element_handle: u64,
    num_children: u32,
) {
    let mut tree = lock();
    if crate::log::verbose() && tree.adds + tree.removes < RAW_LOG_LIMIT {
        logf!(
            "raw[{}] {} parent=0x{:x} child=0x{:x} idx={} handle=0x{:x}{} kids={} type={:?} name={:?}",
            tree.adds + tree.removes,
            if added { "ADD" } else { "REM" },
            parent,
            child,
            child_index,
            element_handle,
            if element_handle == child { "" } else { "  (relation.child differs)" },
            num_children,
            type_name,
            name
        );
    }
    // Identity is the element handle; `relation` only supplies the parent link.
    let child = if element_handle != 0 { element_handle } else { child };
    if added {
        tree.adds += 1;
        let seq = tree.seq;
        tree.seq += 1;
        tree.link(child, Node { parent, child_index, type_name, name, seq });
    } else {
        tree.removes += 1;
        tree.unlink(child);
    }
    tree.last_event = Some(Instant::now());
    tree.dirty = true;
}

/// Whether the event stream has been silent for at least `period`: the guard that keeps XAML
/// mutations out of a burst (a `put_Content` mid-stream never returns and wedges the taskbar).
/// `false` before any event has arrived.
pub fn quiet_for(period: Duration) -> bool {
    let tree = lock();
    tree.last_event.is_some_and(|at| at.elapsed() >= period)
}

// Query helpers over the recorded tree.

/// Every recorded element of the given XAML type, in the order they arrived.
pub fn find_by_type(type_name: &str) -> Vec<u64> {
    let tree = lock();
    let mut hits: Vec<(u64, u64)> = tree
        .by_type
        .get(type_name)
        .into_iter()
        .flatten()
        .filter_map(|handle| tree.nodes.get(handle).map(|node| (node.seq, *handle)))
        .collect();
    hits.sort_unstable();
    hits.into_iter().map(|(_, handle)| handle).collect()
}

/// Recorded children of a handle, in the child index order XAML reported.
pub fn children_of(parent: u64) -> Vec<u64> {
    let tree = lock();
    let mut kids: Vec<(u32, u64, u64)> = tree
        .children
        .get(&parent)
        .into_iter()
        .flatten()
        .filter_map(|handle| tree.nodes.get(handle).map(|node| (node.child_index, node.seq, *handle)))
        .collect();
    kids.sort_unstable();
    kids.into_iter().map(|(_, _, handle)| handle).collect()
}

/// Every recorded element carrying the given `x:Name`.
pub fn find_by_name(name: &str) -> Vec<u64> {
    let tree = lock();
    tree.by_name.get(name).map(|set| set.iter().copied().collect()).unwrap_or_default()
}

/// How many elements have been announced: it moves whenever the tree grows (a part rebuilt adds
/// its elements again), so what was found in it can be kept until it moves.
pub fn version() -> u64 {
    lock().seq
}

/// When a handle was announced, as a monotonic sequence number.
///
/// Tells a live element from a stale one whose `Remove` never arrived: between candidates of the
/// same name or type, the newest is the live one.
pub fn seq_of(handle: u64) -> Option<u64> {
    let tree = lock();
    tree.nodes.get(&handle).map(|node| node.seq)
}

/// The most recently announced of `candidates`.
pub fn newest(candidates: impl IntoIterator<Item = u64>) -> Option<u64> {
    candidates
        .into_iter()
        .filter_map(|handle| Some((seq_of(handle)?, handle)))
        .max()
        .map(|(_, handle)| handle)
}

/// The recorded `x:Name` of a handle.
pub fn name_of(handle: u64) -> Option<String> {
    let tree = lock();
    tree.nodes.get(&handle).map(|node| node.name.clone())
}

/// The recorded XAML type name of a handle.
pub fn type_of(handle: u64) -> Option<String> {
    let tree = lock();
    tree.nodes.get(&handle).map(|node| node.type_name.clone())
}

/// The recorded parent of a handle.
pub fn parent_of(handle: u64) -> Option<u64> {
    let tree = lock();
    tree.nodes.get(&handle).map(|node| node.parent)
}

// Walkers over the recorded tree (and one over the live one).

/// Climbs the recorded tree looking for an ancestor of a given XAML type.
pub fn ancestor_of_type(start: u64, wanted: &str, max_up: usize) -> Option<u64> {
    let mut handle = start;
    for _ in 0..max_up {
        handle = parent_of(handle)?;
        if type_of(handle).as_deref() == Some(wanted) {
            return Some(handle);
        }
    }
    None
}

/// The first element named `name` under `root`, nearest first.
pub fn named(root: u64, name: &str) -> Option<u64> {
    descendants(root).into_iter().find(|h| name_of(*h).as_deref() == Some(name))
}

/// Every recorded element under `root`, nearest first (bounded: a tray button holds a few dozen).
pub fn descendants(root: u64) -> Vec<u64> {
    // Kept until the tree grows: each clock tick asks for the same few (the bell's, a section's)
    // several times, and a walk locks the tree and sorts at every node. A handle gone since is
    // harmless (reading through it finds nothing); a part rebuilt adds elements, so a new walk.
    /// The tree's version, then each root walked and what was under it.
    type Found = (u64, Vec<(u64, Vec<u64>)>);
    static FOUND: Mutex<Found> = Mutex::new((u64::MAX, Vec::new()));
    let version = version();
    let mut found = crate::lock(&FOUND);
    if found.0 != version {
        *found = (version, Vec::new());
    }
    if let Some((_, kept)) = found.1.iter().find(|(r, _)| *r == root) {
        return kept.clone();
    }
    let walked = walk(root);
    found.1.push((root, walked.clone()));
    walked
}

/// Every element under `root`, breadth first (400 at most).
fn walk(root: u64) -> Vec<u64> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from(children_of(root));
    while let Some(handle) = queue.pop_front() {
        if found.len() >= 400 {
            break;
        }
        found.push(handle);
        queue.extend(children_of(handle));
    }
    found
}

/// Nearest descendant of `icon` whose runtime class is `target`, walked in the **live** tree (the
/// recorded one misses re-parented children; see FINDINGS.md, "The presenter must be found live").
/// Breadth-first matters: for `Border` it must find the shell's `BackgroundBorder` before our
/// (deeper) pill.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn descendant_of_class(diagnostics: &crate::xamlom::IXamlDiagnostics, icon: u64, target: &str) -> Option<u64> {
    use crate::winrt::{IDependencyObject, IVisualTreeHelperStatics, VISUAL_TREE_HELPER};
    use windows::Win32::Foundation::S_OK;
    use windows_core::{IInspectable, Interface};
    /// Bounds the work done on the shell's UI thread (the presenter is two levels down).
    const MAX_DEPTH: usize = 6;
    /// Hard stop, so an unexpected tree cannot spin inside Explorer.
    const MAX_VISITED: usize = 256;

    let statics: IVisualTreeHelperStatics = crate::xaml::factory(VISUAL_TREE_HELPER)?;
    // Must be a `DependencyObject`, not the plain `IInspectable` (wrong vtable).
    let root = crate::xaml::element::<IDependencyObject>(diagnostics, icon)?;

    let mut frontier = vec![root];
    let mut visited = 0usize;
    for _ in 0..MAX_DEPTH {
        let mut next: Vec<IDependencyObject> = Vec::new();
        for parent in &frontier {
            let mut count = 0i32;
            if statics.GetChildrenCount(parent.as_raw(), &mut count) != S_OK {
                continue;
            }
            for index in 0..count {
                if visited >= MAX_VISITED {
                    logf!("descendant_of_class: gave up after {MAX_VISITED} elements");
                    return None;
                }
                visited += 1;
                let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
                if statics.GetChild(parent.as_raw(), index, &mut raw) != S_OK || raw.is_null() {
                    continue;
                }
                // Owned: dropping it releases `GetChild`'s reference.
                let child = IInspectable::from_raw(raw);
                if child.GetRuntimeClassName().is_ok_and(|name| name == target) {
                    let mut handle = 0u64;
                    if diagnostics.GetHandleFromIInspectable(child.as_raw(), &mut handle) == S_OK && handle != 0 {
                        return Some(handle);
                    }
                }
                if let Ok(node) = child.cast::<IDependencyObject>() {
                    next.push(node);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    None
}

/// Starts the dump watchdog exactly once, in verbose mode only (a diagnostic aid, and the bulk
/// of the log).
pub fn start_watchdog() {
    if !crate::log::verbose() {
        return;
    }
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(POLL_INTERVAL);
        let ready = {
            let tree = lock();
            tree.dirty
                && tree
                    .last_event
                    .is_some_and(|at| at.elapsed() >= QUIET_PERIOD)
        };
        if ready {
            dump();
        }
    });
}

/// Renders the recorded tree under the lock, then logs it after releasing it: the UI-thread
/// callback (`record`) waits on this lock, so it is never held across output.
fn dump() {
    let mut out = Vec::new();
    {
        let mut tree = lock();
        tree.dirty = false;
        tree.dumps += 1;

        out.push(format!(
            "===== visual tree dump #{} — {} nodes live ({} adds, {} removes) =====",
            tree.dumps,
            tree.nodes.len(),
            tree.adds,
            tree.removes
        ));

        // Children bucketed by parent, ordered by the index XAML reported.
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        for (&handle, node) in &tree.nodes {
            children.entry(node.parent).or_default().push(handle);
        }
        for bucket in children.values_mut() {
            bucket.sort_by_key(|h| {
                let node = &tree.nodes[h];
                (node.child_index, node.seq)
            });
        }

        // A root is any node whose parent we never saw (usually parent == 0).
        let mut roots: Vec<u64> = tree
            .nodes
            .iter()
            .filter(|(_, node)| !tree.nodes.contains_key(&node.parent))
            .map(|(&handle, _)| handle)
            .collect();
        roots.sort_by_key(|h| tree.nodes[h].seq);

        for root in roots {
            render_subtree(&tree, &children, root, 0, &mut out);
        }
        out.push(format!("===== end of dump #{} =====", tree.dumps));
    }
    for text in out {
        logf!("{text}");
    }
    logf!("log file: {}", crate::log::path().display());
}

fn render_subtree(
    tree: &Tree,
    children: &HashMap<u64, Vec<u64>>,
    handle: u64,
    depth: usize,
    out: &mut Vec<String>,
) {
    // Explorer's tree is nowhere near this deep; the guard is purely so a cycle
    // introduced by a bad handle can't spin forever inside the shell.
    if depth > 64 {
        out.push(format!("{}… depth limit", "  ".repeat(depth)));
        return;
    }
    let Some(node) = tree.nodes.get(&handle) else {
        return;
    };
    let named = if node.name.is_empty() {
        String::new()
    } else {
        format!("#{}", node.name)
    };
    out.push(format!(
        "{}{}{}  [0x{:x}]",
        "  ".repeat(depth),
        node.type_name,
        named,
        handle
    ));
    if let Some(bucket) = children.get(&handle) {
        for &child in bucket {
            render_subtree(tree, children, child, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: u64, type_name: &str, name: &str, seq: u64) -> Node {
        Node { parent, child_index: 0, type_name: type_name.into(), name: name.into(), seq }
    }

    #[test]
    fn indexes_follow_adds_moves_and_removes() {
        let mut tree = Tree::default();
        tree.link(10, node(1, "Grid", "A", 0));
        tree.link(11, node(1, "Border", "", 1));
        assert_eq!(tree.children[&1].len(), 2);
        // Re-announced under another parent with another name: old index entries go.
        tree.link(10, node(2, "Grid", "B", 2));
        assert_eq!(tree.children[&1].iter().copied().collect::<Vec<_>>(), vec![11]);
        assert!(tree.children[&2].contains(&10));
        assert!(!tree.by_name.contains_key("A"));
        assert!(tree.by_name["B"].contains(&10));
        tree.unlink(10);
        tree.unlink(11);
        assert!(tree.children.is_empty() && tree.by_type.is_empty() && tree.by_name.is_empty());
        assert!(!tree.by_name.contains_key(""), "unnamed elements are not indexed by name");
    }
}
