# rust-memcache

Memcached client for rust, using [libmemcached](http://libmemcached.org/) and rust FFI;

* travis-ci: [![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache)
* crates.io: [memcache](https://crates.io/crates/memcache)

## Dependences

- brew install libmemcached

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

- [ ] Build on Linux
- [ ] More commands
- [ ] Multi server support
- [ ] Typed interface
- [ ] Memory leak check
- [ ] Documents

# License

MIT
