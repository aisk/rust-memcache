# rust-memcache
[![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache) [![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)

Memcached client for rust.

## Usage
```rust
let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
conn.flush().unwrap();
let value = &memcache::value::Value{bytes: b"bar", flags: 0};
conn.set("foo", value, 42).unwrap();
conn.get(&["foo", "bar", "baz"]).unwrap();
```

## TODO

- [ ] Ascii protocal
- [ ] Binary protocal
- [ ] Multi server support
- [ ] Typed interface
- [ ] Documents

# License

MIT
