//! A global allocator that tracks live bytes, the peak since the last reset and the largest single
//! allocation, so the fuzzer can flag inputs that make a parser allocate far more than expected.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub struct Tracking;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);

fn grew(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
    LARGEST.fetch_max(size, Relaxed);
}

// SAFETY: forwards every call to the system allocator unchanged; only counters are added.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
                LARGEST.fetch_max(new_size, Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
        }
        p
    }
}

/// Start a measurement window: the peak restarts from the bytes live now.
pub fn reset() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    LARGEST.store(0, Relaxed);
    live
}

/// `(peak above the live bytes at reset, largest single allocation)` since [`reset`].
pub fn window(base: usize) -> (usize, usize) {
    (
        PEAK.load(Relaxed).saturating_sub(base),
        LARGEST.load(Relaxed),
    )
}
