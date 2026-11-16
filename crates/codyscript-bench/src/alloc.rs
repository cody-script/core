//! Memory measurement: a global allocator that counts.
//!
//! A benchmark binary opts in with
//! `#[global_allocator] static ALLOC: Counting = Counting;`, then wraps the
//! work in [`measure`]. It counts every allocation in the process, across
//! threads, so measure one thing at a time.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// The system allocator, counting.
pub struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(bytes: usize) {
    ALLOCATIONS.fetch_add(1, Relaxed);
    ALLOCATED.fetch_add(bytes, Relaxed);
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every call is forwarded to `System` unchanged; this only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = System.realloc(ptr, layout, new_size);
        if !new.is_null() {
            // Counted as freeing the old block and allocating the new one.
            LIVE.fetch_sub(layout.size(), Relaxed);
            grew(new_size);
        }
        new
    }
}

/// What some work allocated.
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    /// Allocations made (a reallocation counts as one).
    pub allocations: usize,
    /// Bytes those allocations asked for, in total.
    pub allocated: usize,
    /// The most memory live at once, above what was live before.
    pub peak: usize,
    /// Memory still live afterwards, in what the work returned: its size.
    pub retained: isize,
}

/// Runs `work`, returning its result and what it allocated. Only
/// meaningful when [`Counting`] is the global allocator.
pub fn measure<T>(work: impl FnOnce() -> T) -> (T, Usage) {
    let (allocations, allocated, live) = (
        ALLOCATIONS.load(Relaxed),
        ALLOCATED.load(Relaxed),
        LIVE.load(Relaxed),
    );
    PEAK.store(live, Relaxed);
    let result = work();
    let usage = Usage {
        allocations: ALLOCATIONS.load(Relaxed) - allocations,
        allocated: ALLOCATED.load(Relaxed) - allocated,
        peak: PEAK.load(Relaxed) - live,
        retained: LIVE.load(Relaxed) as isize - live as isize,
    };
    (result, usage)
}

/// Bytes, readably: `512 B`, `3.4 KiB`, `1.2 MiB`.
pub fn bytes(n: isize) -> String {
    let size = n.unsigned_abs() as f64;
    let sign = if n < 0 { "-" } else { "" };
    if size < 1024.0 {
        format!("{sign}{size} B")
    } else if size < 1024.0 * 1024.0 {
        format!("{sign}{:.1} KiB", size / 1024.0)
    } else {
        format!("{sign}{:.1} MiB", size / (1024.0 * 1024.0))
    }
}

/// The memory report: a Markdown table, one row per scenario.
pub fn report(rows: &[(&str, Usage)]) -> String {
    let mut table = String::from(
        "| Scenario | Allocations | Allocated | Peak | Retained |\n|---|--:|--:|--:|--:|\n",
    );
    for (name, usage) in rows {
        table.push_str(&format!(
            "| {name} | {} | {} | {} | {} |\n",
            usage.allocations,
            bytes(usage.allocated as isize),
            bytes(usage.peak as isize),
            bytes(usage.retained),
        ));
    }
    table
}
