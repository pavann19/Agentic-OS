//! Kernel heap. Phase 0 item — nothing in the C kernel had this at all
//! (`docs/ROADMAP.md` explicitly lists it as not-yet-existing, not as a
//! port target). Implements `GlobalAlloc` and is installed as
//! `#[global_allocator]` in `main.rs`, so kernel code can use `alloc`'s
//! `Vec`/`Box`/`BTreeMap` etc. going forward — the more capable choice
//! over a bump-only allocator that would block that off, at the cost of
//! a real free-list instead of a simpler always-grow one.
//!
//! Design: a singly-linked free list of variably-sized blocks (the classic
//! "linked list allocator" shape — same idea as the `linked_list_allocator`
//! crate, hand-written here to keep the no-external-crates discipline).
//! `alloc()` first-fits a free block, splitting off the remainder if it's
//! big enough to be useful; `dealloc()` pushes the freed block back onto
//! the list. No coalescing of adjacent free blocks yet — a real gap (see
//! `PHASE0_PROGRESS.md`), acceptable for Phase 0 since nothing yet
//! allocates and frees enough, in a long-running enough kernel, for
//! fragmentation to matter. Revisit before Phase 1's scheduler does
//! sustained alloc/free churn.
//!
//! Real bug found and fixed (see `critical.rs`'s doc comment for the full
//! investigation, triggered by a reproducible late-boot crash under
//! sustained multi-process scheduling): this module's own doc used to
//! read "Not thread/interrupt-safe by itself... revisit with real
//! locking before Phase 1 [the scheduler] does [sustained alloc/free
//! churn]" — Phase 1 landed a long time before this was actually
//! revisited. A `SpinMutex` alone was never going to be enough here
//! either: on a single core with PREEMPTIVE scheduling, a thread
//! preempted WHILE HOLDING the lock (mid free-list mutation) can
//! deadlock the scheduler itself if `schedule()` — running inside the
//! very timer interrupt that preempted it — ever needs to allocate (its
//! own `VecDeque<Box<Thread>>` growing, for instance): nothing could ever
//! release that lock, since doing so requires resuming the very thread
//! the stuck interrupt handler is blocking on. `GlobalAlloc::alloc`/
//! `dealloc` below now wrap their entire lock-acquire-mutate-release
//! sequence in `critical::without_interrupts` — no preemption can occur
//! while any of this runs, so neither hazard is reachable anymore.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;

use crate::klog_info;
use crate::pmm;
use crate::vmm;

pub const HEAP_VIRTUAL_BASE: u64 = 0xFFFF_FF00_0000_0000;
const INITIAL_HEAP_PAGES: u64 = 2048; // 8MB to start; grow_by() can extend later.

struct FreeListNode {
    size: usize,
    next: *mut FreeListNode,
}

pub struct LockedHeap {
    head: *mut FreeListNode,
    heap_end: u64, // one past the last mapped byte — used by grow_by()
}

unsafe impl Sync for LockedHeap {} // single-threaded so far; see module doc.

impl LockedHeap {
    const fn empty() -> Self {
        LockedHeap {
            head: null_mut(),
            heap_end: 0,
        }
    }

    unsafe fn add_free_region(&mut self, addr: u64, size: usize) {
        if size < core::mem::size_of::<FreeListNode>() {
            return; // too small to track — a real (small) leak, acceptable for now.
        }
        let node = addr as *mut FreeListNode;
        (*node).size = size;
        (*node).next = self.head;
        self.head = node;
    }

    fn align_up(addr: usize, align: usize) -> usize {
        (addr + align - 1) & !(align - 1)
    }

    /// First-fit search: returns (block_addr, actual_usable_size) after
    /// unlinking it from the free list, splitting off a remainder region
    /// back onto the list if what's left over is big enough to bother
    /// tracking.
    unsafe fn find_and_remove(&mut self, size: usize, align: usize) -> Option<(u64, usize)> {
        let mut prev: *mut *mut FreeListNode = &mut self.head;
        let mut current = self.head;
        while !current.is_null() {
            let node_addr = current as u64;
            let alloc_start = Self::align_up(node_addr as usize, align) as u64;
            let alloc_end = alloc_start + size as u64;
            let node_end = node_addr + (*current).size as u64;

            if alloc_end <= node_end {
                let node_size = (*current).size;
                let next = (*current).next;
                *prev = next; // unlink

                let front_pad = alloc_start - node_addr;
                if front_pad > 0 {
                    // Region before the aligned start is still free —
                    // re-add it (only reachable if front_pad is at least
                    // large enough to hold a FreeListNode, guaranteed by
                    // caller's minimum alignment/size expectations in
                    // practice; a stray few unusable bytes if not is the
                    // same acceptable small leak as the size check above).
                    self.add_free_region(node_addr, front_pad as usize);
                }
                let back_pad = node_end - alloc_end;
                if back_pad > 0 {
                    self.add_free_region(alloc_end, back_pad as usize);
                }
                let _ = node_size;
                return Some((alloc_start, size));
            }
            prev = &mut (*current).next;
            current = (*current).next;
        }
        None
    }

    /// Maps `pages` more pages onto the end of the heap window and adds
    /// them as one new free region. Called once at init with
    /// INITIAL_HEAP_PAGES; can be called again later if the heap runs out
    /// (not wired to an automatic trigger yet — see alloc()'s OOM path).
    pub unsafe fn grow_by(&mut self, pages: u64) {
        let start = self.heap_end;
        for p in 0..pages {
            let phys = pmm::alloc_page();
            if phys == 0 {
                klog_info!("HEAP: out of physical pages while growing heap");
                return;
            }
            vmm::map_heap_page(start + p * pmm::PAGE_SIZE, phys);
        }
        self.heap_end += pages * pmm::PAGE_SIZE;
        self.add_free_region(start, (pages * pmm::PAGE_SIZE) as usize);
    }
}

unsafe impl GlobalAlloc for spin_shim::SpinMutex<LockedHeap> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        crate::critical::without_interrupts(|| {
            let size = layout.size().max(core::mem::size_of::<FreeListNode>());
            let align = layout.align().max(8);
            let mut guard = self.lock();
            match unsafe { guard.find_and_remove(size, align) } {
                Some((addr, _)) => addr as *mut u8,
                None => null_mut(),
            }
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        crate::critical::without_interrupts(|| {
            let size = layout.size().max(core::mem::size_of::<FreeListNode>());
            let mut guard = self.lock();
            unsafe { guard.add_free_region(ptr as u64, size) };
        })
    }
}

/// Minimal spinlock, hand-written to avoid an external crate dependency —
/// see the "no external crates" discipline noted in kernel_rs/Cargo.toml.
/// Uncontended in practice today (single-threaded kernel, no preemption
/// yet), but real `lock`/`unlock` semantics via `compare_exchange` so this
/// doesn't need revisiting once a scheduler exists — only the "is it ever
/// actually contended" assumption changes then, not this code.
mod spin_shim {
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicBool, Ordering};

    pub struct SpinMutex<T> {
        locked: AtomicBool,
        data: UnsafeCell<T>,
    }

    unsafe impl<T> Sync for SpinMutex<T> {}

    pub struct SpinGuard<'a, T> {
        lock: &'a SpinMutex<T>,
    }

    impl<T> SpinMutex<T> {
        pub const fn new(data: T) -> Self {
            SpinMutex {
                locked: AtomicBool::new(false),
                data: UnsafeCell::new(data),
            }
        }

        pub fn lock(&self) -> SpinGuard<'_, T> {
            while self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
            SpinGuard { lock: self }
        }
    }

    impl<'a, T> core::ops::Deref for SpinGuard<'a, T> {
        type Target = T;
        fn deref(&self) -> &T {
            unsafe { &*self.lock.data.get() }
        }
    }
    impl<'a, T> core::ops::DerefMut for SpinGuard<'a, T> {
        fn deref_mut(&mut self) -> &mut T {
            unsafe { &mut *self.lock.data.get() }
        }
    }
    impl<'a, T> Drop for SpinGuard<'a, T> {
        fn drop(&mut self) {
            self.lock.locked.store(false, Ordering::Release);
        }
    }
}

#[global_allocator]
static ALLOCATOR: spin_shim::SpinMutex<LockedHeap> = spin_shim::SpinMutex::new(LockedHeap::empty());

pub fn init() {
    // Runs before the kernel's first `sti` (main.rs enables interrupts
    // well after this), so this particular call isn't actually exposed
    // to the race critical.rs documents -- wrapped anyway for the same
    // reason every other allocator entry point now is: consistency, and
    // safety against this ordering ever changing later.
    crate::critical::without_interrupts(|| unsafe {
        let mut guard = ALLOCATOR.lock();
        guard.heap_end = HEAP_VIRTUAL_BASE;
        guard.grow_by(INITIAL_HEAP_PAGES);
    });
    klog_info!(
        "Heap initialized: {} MB at 0x{:x}",
        (INITIAL_HEAP_PAGES * pmm::PAGE_SIZE) / (1024 * 1024),
        HEAP_VIRTUAL_BASE
    );
}

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    crate::klog::panic("kernel heap allocation failed");
    #[allow(unreachable_code)]
    {
        let _ = layout;
        loop {}
    }
}
