#![feature(ptr_internals)]
#![feature(allocator_api)]
#![feature(alloc_layout_extra)]
#![feature(slice_ptr_get)]

extern crate lazy_static;
mod allocator_vec;
mod server;
mod stats;

use stats::{StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;

#[global_allocator]

static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn main() {
    // Memory map serve
    server::create_server_of_memory_map().unwrap();
}
