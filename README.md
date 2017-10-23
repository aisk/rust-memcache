# rust-memcache
[![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache)
[![Coverage Status](https://coveralls.io/repos/github/aisk/rust-memcache/badge.svg?branch=master)](https://coveralls.io/github/aisk/rust-memcache?branch=master)
[![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Docs](https://docs.rs/memcache/badge.svg)](https://docs.rs/memcache/)

rust-memcache is a [Memcached](https://memcached.org/) client written in pure rust.

## Install:

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "*"
```

## Features:

- [x] Binary protocal
- [x] TCP connection
- [ ] UDP connection
- [x] UNIX Domain socket connection
- [ ] Automatically compress
- [ ] Automatically serialize to JSON / msgpack etc.
- [x] Typed interface
- [ ] Mutiple server support with custom key hash algorithm

## Basic usage:

```rust
// create connection
let mut client = memcache::Client::connect("memcache://127.0.0.1:12345").unwrap();
// or using unix domain socket:
// let mut client = memcache::Client::connect("memcache:///tmp/memcached.sock").unwrap();

// flush the database
client.flush().unwrap();

// set a string value
client.set("foo", "bar").unwrap();
// retrieve from memcached
let value: String = client.get("foo").unwrap();
assert!(value == "bar");
```

## License

MIT
