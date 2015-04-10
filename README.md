# rust-memcache

Memcached client for rust.

* travis-ci: [![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache)
* crates.io: [memcache](https://crates.io/crates/memcache)

## Usage
```rust
let mut conn = Connection::connect("localhost", 2333).unwrap();

conn.set("foo", b"bar", 0).unwrap();
assert!{ conn.get("foo").unwrap().unwrap().as_slice() == b"bar" };
```

# License

MIT
