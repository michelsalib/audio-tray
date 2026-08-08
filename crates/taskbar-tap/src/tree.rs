//! Accumulates `OnVisualTreeChange` events into a tree and dumps it.
//!
//! `AdviseVisualTreeChange` replays the entire existing tree as a burst of `Add`
//! mutations before switching to live deltas, so "the burst went quiet" is the
//! signal that a complete snapshot has arrived. A watchdog thread waits for that
//! quiet period and writes an indented dump; if mutations resume it re-arms and
//! dumps again, which is how you watch a flyout open or the taskbar re-theme.

use crate::log::logf;
use std::collections::HashMap;
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
    adds: u64,
    removes: u64,
    seq: u64,
    last_event: Option<Instant>,
    dirty: bool,
    dumps: u32,
}

fn tree() -> &'static Mutex<Tree> {
    static TREE: OnceLock<Mutex<Tree>> = OnceLock::new();
    TREE.get_or_init(|| Mutex::new(Tree::default()))
}

fn lock() -> std::sync::MutexGuard<'static, Tree> {
    match tree().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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
        tree.nodes.insert(
            child,
            Node {
                parent,
                child_index,
                type_name,
                name,
                seq,
            },
        );
    } else {
        tree.removes += 1;
        tree.nodes.remove(&child);
    }
    tree.last_event = Some(Instant::now());
    tree.dirty = true;
}

/// Whether the event stream has been silent for at least `period`.
///
/// This is the guard that keeps XAML mutations out of the initial replay burst.
/// While `AdviseVisualTreeChange` is streaming, the UI thread is inside a
/// marshalled call, and calling `put_Content` against a tray element from there
/// **never returns** — it blocks Explorer's UI thread outright, taskbar and all.
/// Waiting for quiet is what makes the mutation safe.
///
/// `false` before any event has arrived: there is nothing recorded to act on yet.
pub fn quiet_for(period: Duration) -> bool {
    let tree = lock();
    tree.last_event.is_some_and(|at| at.elapsed() >= period)
}

// Query helpers over the recorded tree. `type_of` / `parent_of` drive the current
// decoration path; the rest are the vocabulary any richer selector work will need,
// and are kept rather than re-derived later.

/// Every recorded element of the given XAML type, in the order they arrived.
#[allow(dead_code)]
pub fn find_by_type(type_name: &str) -> Vec<u64> {
    let tree = lock();
    let mut hits: Vec<(u64, u64)> = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.type_name == type_name)
        .map(|(&handle, node)| (node.seq, handle))
        .collect();
    hits.sort_unstable();
    hits.into_iter().map(|(_, handle)| handle).collect()
}

/// Recorded children of a handle, in the child index order XAML reported.
pub fn children_of(parent: u64) -> Vec<u64> {
    let tree = lock();
    let mut kids: Vec<(u32, u64, u64)> = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.parent == parent)
        .map(|(&handle, node)| (node.child_index, node.seq, handle))
        .collect();
    kids.sort_unstable();
    kids.into_iter().map(|(_, _, handle)| handle).collect()
}

/// Every recorded element carrying the given `x:Name`.
pub fn find_by_name(name: &str) -> Vec<u64> {
    let tree = lock();
    tree.nodes
        .iter()
        .filter(|(_, node)| node.name == name)
        .map(|(&handle, _)| handle)
        .collect()
}

/// When a handle was announced, as a monotonic sequence number.
///
/// **For telling a live element from one XAML never told us it had removed.** The recorder drops a
/// node on a `Remove`, but those do not always arrive — a rebuilt subtree can leave the old elements
/// behind, indistinguishable by name or type from the new ones. Between two candidates the newest is
/// the live one, and this is the only thing recorded that says which that is.
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

/// The first descendant of `root` carrying the given `x:Name`.
#[allow(dead_code)]
pub fn find_descendant_named(root: u64, name: &str) -> Option<u64> {
    let tree = lock();
    // Breadth-first so the shallowest match wins — nested templates reuse names.
    let mut frontier = vec![root];
    for _ in 0..MAX_SEARCH_DEPTH {
        let mut next = Vec::new();
        for (&handle, node) in &tree.nodes {
            if !frontier.contains(&node.parent) {
                continue;
            }
            if node.name == name {
                return Some(handle);
            }
            next.push(handle);
        }
        if next.is_empty() {
            return None;
        }
        frontier = next;
    }
    None
}

#[allow(dead_code)]
const MAX_SEARCH_DEPTH: usize = 32;

/// Handles the live feed reported with no parent — the tree roots.
#[allow(dead_code)]
pub fn root_handles() -> Vec<u64> {
    let tree = lock();
    tree.nodes
        .iter()
        .filter(|(_, node)| node.parent == 0)
        .map(|(&handle, _)| handle)
        .collect()
}

/// Starts the dump watchdog exactly once.
///
/// Only in verbose mode: the dumps are the bulk of the log, and the tree they print
/// is a spike-exploration aid rather than something the strip needs.
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

fn dump() {
    let mut tree = lock();
    tree.dirty = false;
    tree.dumps += 1;

    logf!(
        "===== visual tree dump #{} — {} nodes live ({} adds, {} removes) =====",
        tree.dumps,
        tree.nodes.len(),
        tree.adds,
        tree.removes
    );

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
        write_subtree(&tree, &children, root, 0);
    }
    logf!("===== end of dump #{} =====", tree.dumps);
    logf!("log file: {}", crate::log::path().display());
}

fn write_subtree(tree: &Tree, children: &HashMap<u64, Vec<u64>>, handle: u64, depth: usize) {
    // Explorer's tree is nowhere near this deep; the guard is purely so a cycle
    // introduced by a bad handle can't spin forever inside the shell.
    if depth > 64 {
        logf!("{}… depth limit", "  ".repeat(depth));
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
    logf!(
        "{}{}{}  [0x{:x}]",
        "  ".repeat(depth),
        node.type_name,
        named,
        handle
    );
    if let Some(bucket) = children.get(&handle) {
        for &child in bucket {
            write_subtree(tree, children, child, depth + 1);
        }
    }
}
