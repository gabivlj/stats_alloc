## Memory mapper in Rust
This is a custom allocator modified from https://github.com/neoeinstein/stats_alloc where 
it creates a memory map of the program and also has a simple http server which returns a json
with the information of the program.

Probably could be done cleaner without static variables and a spinlock but for the mission
it works fine. I think there is no use for this in production though! It's just a super fast
way of getting that information.
