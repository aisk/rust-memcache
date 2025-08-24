# rust-memcache

[![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Docs](https://docs.rs/memcache/badge.svg)](https://docs.rs/memcache/)

rust-memcache is a [memcached](https://memcached.org/) client written in pure rust.

![logo](https://cloudflare-ipfs.com/ipfs/QmY2otmZFbrLfCQZ2JG8bsEsMGegHrh8WgupcyTcyoShiS)

## Install

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "*"
```

## Features

- [x] All memcached supported protocols
  - [x] Binary protocol
  - [x] ASCII protocol
- [x] All memcached supported connections
  - [x] TCP connection
  - [x] UDP connection
  - [x] UNIX Domain socket connection
  - [x] TLS connection
- [ ] Encodings
  - [x] Typed interface
  - [ ] Automatically compress
  - [ ] Automatically serialize to JSON / msgpack etc
- [x] Memcached cluster support with custom key hash algorithm
- [x] Authority
  - [x] Binary protocol (plain SASL authority plain)
  - [x] ASCII protocol

## Basic usage

```rust
// create connection with to memcached server node:
let client = memcache::connect("memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true").unwrap();

// flush the database
client.flush().unwrap();

// set a string value
client.set("foo", "bar", 0).unwrap();

// retrieve from memcached:
let value: Option<String> = client.get("foo").unwrap();
assert_eq!(value, Some(String::from("bar")));
assert_eq!(value.unwrap(), "bar");

// prepend, append:
client.prepend("foo", "foo").unwrap();
client.append("foo", "baz").unwrap();
let value: String = client.get("foo").unwrap().unwrap();
assert_eq!(value, "foobarbaz");

// cas(check and set):
let (value, _flags, cas_token): (String, u32, Option<u64>) = client.get("foo").unwrap().unwrap();
assert_eq!(value, "foobarbaz");
let cas_id = cas_token.unwrap();
client.cas("foo", "qux", 0, cas_id).unwrap();

// delete value:
client.delete("foo").unwrap();

// using counter:
client.set("counter", 40, 0).unwrap();
client.increment("counter", 2).unwrap();
let answer: i32 = client.get("counter").unwrap().unwrap();
assert_eq!(answer, 42);
```

## Custom key hash function

If you have multiple memcached server, you can create the `memcache::Client` struct with a vector of urls of them. Which server will be used to store and retrive is based on what the key is.

This library have a basic rule to do this with rust's builtin hash function, and also you can use your custom function to do this, for something like you can using a have more data on one server which have more memory quota, or cluster keys with their prefix, or using consitent hash for large memcached cluster.

```rust
let mut client = memcache::connect(vec!["memcache://127.0.0.1:12345", "memcache:///tmp/memcached.sock"]).unwrap();
client.hash_function = |key: &str| -> u64 {
    // your custom hashing function here
    return 1;
};
```

## Contributing

Before sending pull request, please ensure:

- `cargo fmt` is being run;
- Commit message is using [gitmoji](https://gitmoji.carloscuesta.me/) with first character is lower cased, for example: `:sparkles: rust-memcache can print money now`.

## Contributors

<a href="https://github.com/aisk/rust-memcache/graphs/contributors">
  <img src="https://contributors-img.firebaseapp.com/image?repo=aisk/rust-memcache" />
</a>

## License

MIT
