# rust-memcache
[![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache) [![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)

Memcached client for rust.

## Usage
```rust
let client = memcache::connect("localhost", 2333).unwrap();
client.flush(0).unwrap();

client.set_raw("foo", &[0x1u8, 0x2u8, 0x3u8], 0, 42).unwrap();

let (value, flags) = client.get_raw("foo").unwrap();
assert!(value == &[0x1u8, 0x2u8, 0x3u8]);
assert!(flags == 42);
```

## TODO

- [ ] Ascii protocal
- [ ] Binary protocal
- [ ] Multi server support
- [ ] Typed interface
- [ ] Documents

# License

MIT
