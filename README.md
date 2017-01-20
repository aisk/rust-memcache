# rust-memcache
[![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache)
[![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Docs](https://docs.rs/memcache/badge.svg)](https://docs.rs/memcache/)

rust-memcache is a [Memcached](https://memcached.org/) client written in pure rust.

## Install:

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "~0.1"
```

## Features:

- [x] ASCII protocal
- [ ] Binary protocal
- [x] TCP connection
- [ ] UDP connection
- [ ] UNIX Domain socket connection
- [ ] Automatically compress
- [ ] Automatically serialize to JSON / msgpack etc.
- [x] Typed interface
- [ ] Mutiple server support with custom key hash algorithm

## Basic usage:

```rust
// create connection
let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();

// flush the database
conn.flush().unwrap();

// set a string value
conn.set("foo", "bar").unwrap();
// retrieve from memcached
let value: String = conn.get("foo").unwrap();
assert!(value == "bar");

// set a int value
conn.set("number", 42).unwrap();
// increment it atomic
conn.incr("number", 1);
// retrieve it as i32
let value: i32 = conn.get("number").unwrap();
assert!(value == 43);
```

## License

MIT
