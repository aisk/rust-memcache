# rust-memcache
[![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache) [![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)

Memcached client for rust.

## Usage
```rust
// create connection
let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();

// flush the database
conn.flush().unwrap();

// set a string value with expire time
conn.set("foo", "bar", 42).unwrap();
// retrie from memcached
let value: String = conn.get("foo").unwrap();
assert!(value == "bar");

// set a int value with immortal expire
conn.set("number", 42, 0).unwrap();
// increment it atomic
conn.incr("number", 1);
// retire it as i32
let value: i32 = conn.get("number").unwrap();
assert!(value == 43);
```

## TODO

- [x] Ascii protocal
- [ ] Binary protocal
- [ ] Multi server support
- [x] Typed interface
- [ ] Documents

# License

MIT
