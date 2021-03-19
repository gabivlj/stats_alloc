# stats_alloc

An instrumenting middleware for global allocators in Rust, useful in testing
for validating assumptions regarding allocation patterns, and potentially in
production loads to monitor for memory leaks.


## What is this fork

This fork contains in ./stats_alloc_map a new implementation where it stores a memory map of what you are allocating in your program.
Also contains a server implementation where it serves a HTTP response (always) with the memory map.

## Example

```rust
use stats::{StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;

#[global_allocator]

static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn main() {
    // Will create the server (blocks)
    server::create_server_of_memory_map().unwrap();
}
```


```rust
use stats::{Region, StatsAlloc, INSTRUMENTED_SYSTEM, program_information};
use std::alloc::System;
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
fn main() {
    println!("Memory information: {:?}", program_information);
}
```

## Example response of the server

* Keep in mind that the memory array contains tuples, where the first element is the memory address of 
that memory and the second element is the memory that it occupies.

```json
{
  "length_memory_array":12,
  "memory":[
    [
      140621167745648,
      5
    ],
    [
      140621167745712,
      64
    ],
    [
      140621167745824,
      48
    ],
    [
      140621167745968,
      80
    ],
    [
      140621167746256,
      24
    ],
    [
      140621167746480,
      64
    ],
    [
      140621171951616,
      4096
    ],
    [
      140621168787584,
      24
    ],
    [
      140621168787616,
      64
    ],
    [
      140621180338688,
      1024
    ],
    [
      140621180339712,
      1024
    ],
    [
      140621168788064,
      308
    ]
  ],
  "memory_allocated":6825,
  "total_memory":6842
}
```

## What could be better
It's not a realistic implementation because we use dirty static and dirty AtomicBool techniques to have a non allocating mutex so we don't have a deadlock.
The Region struct is untouched and not refactored to support the new features.