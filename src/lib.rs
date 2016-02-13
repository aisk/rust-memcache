/*!
rust-memcache is a [memcached](http://memcached.org/) client for rust.

It use [libmemcached](http://libmemcached.org/) and rust FFI, so you must have libmemcached installed on your system.

To use rust-memcache, add this to your `Cargo.toml` file:

```Cargo
[dependencies]
memcache = "*"
```

## Connecting to memcached

```rust
let mc = memcache::connect(&("localhost", 2333)).unwrap();
```

Or mutiple memcached servers:
```rust
let mc = memcache::connect(&vec![("localhost", 2333), ("localhost", 2334)]).unwrap();
```
*/

extern crate libc;
extern crate memcached_sys;

mod error;
mod memcache;
mod connectable;

pub use memcache::Memcache;
pub use error::MemcacheError;
pub use error::MemcacheResult;

/// create a memcach::Memcache object.
#[inline]
pub fn connect(p: &connectable::Connectable) -> MemcacheResult<Memcache> {
    return Memcache::connect(p);
}
