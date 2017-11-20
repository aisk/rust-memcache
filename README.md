# rust-memcache

[![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache)
[![Coverage Status](https://coveralls.io/repos/github/aisk/rust-memcache/badge.svg?branch=master)](https://coveralls.io/github/aisk/rust-memcache?branch=master)
[![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Docs](https://docs.rs/memcache/badge.svg)](https://docs.rs/memcache/)

rust-memcache is a [Memcached](https://memcached.org/) client written in pure rust.

## Install

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "*"
```

## Features

- [x] Binary protocal
- [x] TCP connection
- [ ] UDP connection
- [x] UNIX Domain socket connection
- [ ] Automatically compress
- [ ] Automatically serialize to JSON / msgpack etc.
- [x] Typed interface
- [x] Mutiple server support with custom key hash algorithm

## Basic usage

```rust
// create connection with to memcached server node:
let mut client = memcache::Client::new(["memcache://127.0.0.1:12345").unwrap();

// flush the database
client.flush().unwrap();

// set a string value
client.set("foo", "bar", 0).unwrap();

// retrieve from memcached:
let mut value: Option<String> = client.get("foo").unwrap();
assert_eq!(value, Some(String::from("bar")));
assert_eq!(value.unwrap(), "bar");

// prepend, append:
client.prepend("foo", "foo").unwrap();
client.append("foo", "baz").unwrap();
let value: String = client.get("foo").unwrap().unwrap();
assert_eq!(value, "foobarbaz");

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
let mut client = memcache::Client::new(vec!["memcache://127.0.0.1:12345", "memcache:///tmp/memcached.sock"]).unwrap();
client.hash_function = |key: &str| -> u64 {
    // your custom hashing function here
    return 1;
};
```

## License

MIT
