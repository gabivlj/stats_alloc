//! This code is a modified version of https://github.com/neoeinstein/stats_alloc
//! created by @neoeinstein and Albert Liu, please give them all props about this! :)
//! I modified the code so it supports a memory map of the program and make a deterministic form
//! how many bytes your program is occupying,
//!
//! It also has a server in the module `server` that simply returns this memory map in a JSON
//!
//! An instrumenting middleware for global allocators in Rust, useful in testing
//! for validating assumptions regarding allocation patterns, and potentially in
//! production loads to monitor for memory leaks.
//!
//! ## Example
//!
//! ```
//! extern crate stats_alloc;
//!
//! use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM, };
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
//!
//! fn main() {
//!     println!("{:?}", program_information);
//! }
//! ```

#![cfg_attr(feature = "nightly", feature(const_fn))]
#![cfg_attr(feature = "docs-rs", feature(allocator_api))]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    ops,
    sync::atomic::{spin_loop_hint, AtomicBool, AtomicIsize, AtomicUsize, Ordering},
};

use crate::allocator_vec::SpecialVec;

/// Global "mutex", we use an atomic bool because it doesn't allocate
pub static mut ADQUIRED: AtomicBool = AtomicBool::new(false);

/// Super dangerous vector, if for some reason we use the global allocator for this thing
/// the program is gonna crash and burn.
/// I know this vector should be stored in a struct but, putting it simple, this was the fastest
/// way of knowing if this could work. Maybe in the future I will put it in the Region struct
/// but it would be too much of a refactor, this is just a new functionality!
static mut VECTOR_ALLOCATIONS: Option<SpecialVec<Option<(usize, usize)>>> = None;
static mut STACK_ALLOCS: Option<SpecialVec<usize>> = None;

/// Contains memory information about the program
#[derive(Debug)]
pub struct InfoProgram {
    /// The memory map of the program, at the moment the first key of the element is the memory address and the second key
    /// is the memory occupied in that address
    pub memory_map: SpecialVec<(usize, usize)>,
    /// The memory allocated by the user at the moment
    pub memory_allocated: usize,
    /// The memory allocated by the user + the memory allocated by `stats.rs`
    pub total_memory: usize,
    // total_memory_estimation: usize,
}

/// Retrieves the current program information
pub fn program_information() -> InfoProgram {
    unsafe {
        take_lock();
        let mut size = 0;
        // TODO: make this with_capacity (not doing it now because it's not implemented)
        let mut vec = SpecialVec::new();
        if let Some(v) = &VECTOR_ALLOCATIONS {
            for el in v.iter() {
                if el.is_none() || el.unwrap().0 == 0 {
                    continue;
                }
                size += el.unwrap().1;
                vec.push(el.unwrap());
            }
        }
        free_lock();
        // println!("len: {}", VECTOR_ALLOCATIONS.as_ref().unwrap().len());
        InfoProgram {
            memory_map: vec,
            memory_allocated: size,
            total_memory: size
                + VECTOR_ALLOCATIONS.as_ref().unwrap().cap()
                + STACK_ALLOCS.as_ref().unwrap().cap(),
        }
    }
}

/// Takes the global lock `ADQUIRED`, makes a spinlock.
/// ## Safety
/// This function is unsafe because if you call this function twice in the function without
/// `free_lock` you will be in a deadlock. Another reason is that you should always call free_lock
/// at the end of the scope
unsafe fn take_lock() {
    while ADQUIRED.compare_and_swap(false, true, Ordering::Acquire) {
        spin_loop_hint();
    }
}

/// Frees the global lock `ADQUIRED`
fn free_lock() {
    unsafe {
        ADQUIRED.store(false, Ordering::Release);
    }
}

/// Allocates this pointer and size into the `VECTOR_ALLOCATIONS`, also making use of the `STACK_ALLOC`
/// to keep track of the indexes available to reuse
/// ## Safety
/// This function is unsafe because if you took the global lock `ADQUIRED` this function will be a deadlock
unsafe fn allocate_into_vector(size: usize, ptr: *mut u8) {
    take_lock();
    if VECTOR_ALLOCATIONS.is_none() {
        VECTOR_ALLOCATIONS = Some(SpecialVec::new());
    }
    if let None = STACK_ALLOCS {
        STACK_ALLOCS = Some(SpecialVec::new());
    }
    let vector_allocations = VECTOR_ALLOCATIONS.as_mut().unwrap();
    if let Some(stack) = &mut STACK_ALLOCS {
        if !stack.is_empty() {
            let pos = stack.pop().unwrap();
            vector_allocations[pos] = Some((std::mem::transmute(ptr), size));
        } else {
            vector_allocations.push(Some((std::mem::transmute(ptr), size)));
        }
    }
    free_lock();
}

/// Deletes a memory address from the memory map, replacing it with a None and adding that position to the
/// `STACK_ALLOCS` vector tu reuse that position later.
/// Returns the size of the deleted pointer
/// ## Safety
/// This function is unsafe because if you took the global lock `ADQUIRED` this function will be a deadlock
unsafe fn delete_pointer(ptr: *mut u8) -> Option<usize> {
    take_lock();
    if let Some(vector_allocations) = &mut VECTOR_ALLOCATIONS {
        for (i, element) in vector_allocations.iter_mut().enumerate() {
            if let Some((pointer, _)) = element {
                if *pointer == ptr as usize {
                    let size = element.take().unwrap();
                    STACK_ALLOCS.as_mut().unwrap().push(i);
                    free_lock();
                    return Some(size.1);
                }
            }
        }
    }
    free_lock();
    None
}

/// An instrumenting middleware which keeps track of allocation, deallocation,
/// and reallocation requests to the underlying global allocator.
#[derive(Default, Debug)]
pub struct StatsAlloc<T: GlobalAlloc> {
    allocations: AtomicUsize,
    deallocations: AtomicUsize,
    reallocations: AtomicUsize,
    bytes_allocated: AtomicUsize,
    bytes_deallocated: AtomicUsize,
    bytes_reallocated: AtomicIsize,
    inner: T,
    // VECTOR_ALLOCATIONS: Mutex<Arc<Vec<[u8; 4096]>>>,
}

/// Allocator statistics
#[derive(Clone, Copy, Default, Debug, Hash, PartialEq, Eq)]
pub struct Stats {
    /// Count of allocation operations
    pub allocations: usize,
    /// Count of deallocation operations
    pub deallocations: usize,
    /// Count of reallocation operations
    ///
    /// An example where reallocation may occur: resizing of a `Vec<T>` when
    /// its length would excceed its capacity. Excessive reallocations may
    /// indicate that resizable data structures are being created with
    /// insufficient or poorly estimated initial capcities.
    ///
    /// ```
    /// let mut x = Vec::with_capacity(1);
    /// x.push(0);
    /// x.push(1); // Potential reallocation
    /// ```
    pub reallocations: usize,
    /// Total bytes requested by allocations
    pub bytes_allocated: usize,
    /// Total bytes freed by deallocations
    pub bytes_deallocated: usize,
    /// Total of bytes requested minus bytes freed by reallocations
    ///
    /// This number is positive if the total bytes requested by reallocation
    /// operations is greater than the total bytes freed by reallocations. A
    /// positive value indicates that resizable structures are growing, while
    /// a negative value indicates that such structures are shrinking.
    pub bytes_reallocated: isize,
}

/// An instrumented instance of the system allocator.
pub static INSTRUMENTED_SYSTEM: StatsAlloc<System> = StatsAlloc {
    allocations: AtomicUsize::new(0),
    deallocations: AtomicUsize::new(0),
    reallocations: AtomicUsize::new(0),
    bytes_allocated: AtomicUsize::new(0),
    bytes_deallocated: AtomicUsize::new(0),
    bytes_reallocated: AtomicIsize::new(0),
    inner: System,
};

impl StatsAlloc<System> {
    /// Provides access to an instrumented instance of the system allocator.
    pub const fn system() -> Self {
        StatsAlloc {
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
            reallocations: AtomicUsize::new(0),
            bytes_allocated: AtomicUsize::new(0),
            bytes_deallocated: AtomicUsize::new(0),
            bytes_reallocated: AtomicIsize::new(0),
            inner: System,
        }
    }
}

impl<T: GlobalAlloc> StatsAlloc<T> {
    /// Provides access to an instrumented instance of the given global
    /// allocator.
    #[cfg(feature = "nightly")]
    pub const fn new(inner: T) -> Self {
        StatsAlloc {
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
            reallocations: AtomicUsize::new(0),
            bytes_allocated: AtomicUsize::new(0),
            bytes_deallocated: AtomicUsize::new(0),
            bytes_reallocated: AtomicIsize::new(0),
            inner,
        }
    }

    /// Provides access to an instrumented instance of the given global
    /// allocator.
    #[cfg(not(feature = "nightly"))]
    pub fn new(inner: T) -> Self {
        StatsAlloc {
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
            reallocations: AtomicUsize::new(0),
            bytes_allocated: AtomicUsize::new(0),
            bytes_deallocated: AtomicUsize::new(0),
            bytes_reallocated: AtomicIsize::new(0),
            inner,
            // VECTOR_ALLOCATIONS: Mutex::default(),
        }
    }

    /// Takes a snapshot of the current view of the allocator statistics.
    pub fn stats(&self) -> Stats {
        Stats {
            allocations: self.allocations.load(Ordering::SeqCst),
            deallocations: self.deallocations.load(Ordering::SeqCst),
            reallocations: self.reallocations.load(Ordering::SeqCst),
            bytes_allocated: self.bytes_allocated.load(Ordering::SeqCst),
            bytes_deallocated: self.bytes_deallocated.load(Ordering::SeqCst),
            bytes_reallocated: self.bytes_reallocated.load(Ordering::SeqCst),
        }
    }
}

impl ops::Sub for Stats {
    type Output = Stats;

    fn sub(mut self, rhs: Self) -> Self::Output {
        self -= rhs;
        self
    }
}

impl ops::SubAssign for Stats {
    fn sub_assign(&mut self, rhs: Self) {
        self.allocations -= rhs.allocations;
        self.deallocations -= rhs.deallocations;
        self.reallocations -= rhs.reallocations;
        self.bytes_allocated -= rhs.bytes_allocated;
        self.bytes_deallocated -= rhs.bytes_deallocated;
        self.bytes_reallocated -= rhs.bytes_reallocated;
    }
}

/// A snapshot of the allocation statistics, which can be used to determine
/// allocation changes while the `Region` is alive.
#[derive(Debug)]
pub struct Region<'a, T: GlobalAlloc + 'a> {
    alloc: &'a StatsAlloc<T>,
    initial_stats: Stats,
}

impl<'a, T: GlobalAlloc + 'a> Region<'a, T> {
    /// Creates a new region using statistics from the given instrumented
    /// allocator.
    #[inline]
    pub fn new(alloc: &'a StatsAlloc<T>) -> Self {
        Region {
            alloc,
            initial_stats: alloc.stats(),
        }
    }

    /// Returns the statistics as of instantiation or the last reset.
    #[inline]
    pub fn initial(&self) -> Stats {
        self.initial_stats
    }

    /// Returns the difference between the currently reported statistics and
    /// those provided by `initial()`.
    #[inline]
    pub fn change(&self) -> Stats {
        self.alloc.stats() - self.initial_stats
    }

    /// Returns the difference between the currently reported statistics and
    /// those provided by `initial()`, resetting initial to the latest
    /// reported statistics.
    #[inline]
    pub fn change_and_reset(&mut self) -> Stats {
        let latest = self.alloc.stats();
        let diff = latest - self.initial_stats;
        self.initial_stats = latest;
        diff
    }

    /// Resets the initial initial to the latest reported statistics from the
    /// referenced allocator.
    #[inline]
    pub fn reset(&mut self) {
        self.initial_stats = self.alloc.stats();
    }
}

unsafe impl<'a, T: GlobalAlloc + 'a> GlobalAlloc for &'a StatsAlloc<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        (*self).alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        (*self).dealloc(ptr, layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        (*self).alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        (*self).realloc(ptr, layout, new_size)
    }
}

unsafe impl<T: GlobalAlloc> GlobalAlloc for StatsAlloc<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.bytes_allocated
            .fetch_add(layout.size(), Ordering::SeqCst);
        self.allocations.fetch_add(1, Ordering::SeqCst);
        let ptr = self.inner.alloc(layout);
        allocate_into_vector(layout.size(), ptr);
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.deallocations.fetch_add(1, Ordering::SeqCst);
        self.bytes_deallocated
            .fetch_add(layout.size(), Ordering::SeqCst);
        self.inner.dealloc(ptr, layout);
        delete_pointer(ptr);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.allocations.fetch_add(1, Ordering::SeqCst);
        self.bytes_allocated
            .fetch_add(layout.size(), Ordering::SeqCst);
        let ptr = self.inner.alloc_zeroed(layout);
        allocate_into_vector(layout.size(), ptr);
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.reallocations.fetch_add(1, Ordering::SeqCst);
        if new_size > layout.size() {
            let difference = new_size - layout.size();
            self.bytes_allocated.fetch_add(difference, Ordering::SeqCst);
        } else if new_size < layout.size() {
            let difference = layout.size() - new_size;
            self.bytes_deallocated
                .fetch_add(difference, Ordering::SeqCst);
        }
        self.bytes_reallocated.fetch_add(
            new_size.wrapping_sub(layout.size()) as isize,
            Ordering::SeqCst,
        );
        let new_ptr = self.inner.realloc(ptr, layout, new_size);
        delete_pointer(ptr);
        allocate_into_vector(new_size, new_ptr);
        new_ptr
    }
}
