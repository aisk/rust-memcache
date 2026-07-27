# rust-memcache

[![Crates.io](https://img.shields.io/crates/v/memcache.svg)](https://crates.io/crates/memcache)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Docs](https://docs.rs/memcache/badge.svg)](https://docs.rs/memcache/)

rust-memcache is a [memcached](https://memcached.org/) client written in pure rust.

![logo](https://repository-images.githubusercontent.com/11312685/c184f380-2fc5-11ea-8d95-e0c6509adae7)

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
  - [x] Meta protocol (experimental, in the `exp` module)
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

## Experimental meta protocol client

The `memcache::exp` module contains a new client built on memcached's [meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt). It is experimental and not feature-complete yet, but the main paths are covered: get / set / delete / counters with their protocol options, CAS, typed values, batches, multiple servers, connection pooling, timeouts, and an async client behind the `tokio` feature.

The API may change between minor versions. Within a minor version it is guaranteed to stay compatible, so pin the minor version if you use it:

```ini
[dependencies]
memcache = "0.20"  # >= 0.20.0, < 0.21.0
```

### Typical usage

```rust
use memcache::exp::MetaClient;

let client = MetaClient::connect("127.0.0.1:11211")?;

// Plain operations; options are chained before send():
client.set("foo", "bar").ttl(60).send()?;
let result = client.get("foo").send()?;
assert_eq!(result.value.as_deref(), Some(&b"bar"[..]));
client.delete("foo").send()?;

// Store only when the key does not exist:
client.set("lock", "1").ttl(30).add().send()?;

// Typed values; numbers are stored as decimal ASCII and work with counters:
client.set("visits", 41u64).send()?;
client.increment("visits").send()?;
let visits: Option<u64> = client.get("visits").send()?.decode()?;

// Several operations in one round trip:
use memcache::exp::{Get, Set};
let results = client.run_batch(vec![
    Set::new("a", "1").ttl(60).into(),
    Get::new("b").into(),
])?;
```

Clients are cheap to clone and shareable across threads or tasks; clones share per-server connection pools and configuration. Pool caps and timeouts are set on a builder before connecting and stay fixed for the client's lifetime. In-flight connections per server are capped at 128 by default (`max_connections`), stale idle connections are aged out after 60 seconds (`idle_timeout`), and a pooled connection that died while idle is transparently redialed:

```rust
use std::time::Duration;

let client = MetaClient::builder()
    .max_idle(16)
    .connect_timeout(Some(Duration::from_millis(200)))
    .io_timeout(Some(Duration::from_millis(200)))
    .connect_multiple(["10.0.0.1:11211", "10.0.0.2:11211"])?;
```

The async client has the same surface, behind the `tokio` feature:

```rust
use memcache::exp::AsyncMetaClient;

let client = AsyncMetaClient::connect("127.0.0.1:11211").await?;
client.set("foo", "bar").ttl(60).send().await?;
let result = client.get("foo").send().await?;
```

### Advanced: cache stampede protection with leases

When a hot key misses or is close to expiring, `lease_ttl` grants exactly one reader a lease to recompute the value; concurrent readers are told the refresh is pending (or keep serving the current value) instead of all hitting the backing store at once:

```rust
let result = client.get("hot").lease_ttl(30).refresh_before(10).send()?;
if result.won_lease() {
    // This reader owns the refresh; recompute and fill the placeholder.
    let fresh = expensive_recompute();
    client
        .set("hot", fresh)
        .ttl(300)
        .compare_cas(result.item.cas.unwrap())
        .send()?;
} else if result.hit() {
    // Serve the current value; another reader is refreshing it.
} else {
    // Miss with the lease held elsewhere: result.lease_busy() is true and
    // the refresh is in flight; fall back or retry shortly.
}
```

### Advanced: optimistic concurrency with CAS

```rust
use memcache::exp::{Meta, MutationStatus};

let current = client.get("profile").meta(Meta::NONE.cas()).send()?;
let updated = mutate(current.value.as_deref());
let stored = client
    .set("profile", updated)
    .compare_cas(current.item.cas.unwrap())
    .send()?;
if stored.status == MutationStatus::CasMismatch {
    // Someone else updated the key in between; reload and retry.
}
```

## Contributing

Before sending pull request, please ensure:

- `cargo fmt` is being run;
- Commit message is using [gitmoji](https://gitmoji.carloscuesta.me/), for example: `✨ rust-memcache can print money now`.

## Contributors

<a href="https://github.com/aisk/rust-memcache/graphs/contributors">
  <img src="https://contributors-img.firebaseapp.com/image?repo=aisk/rust-memcache" />
</a>

## License

MIT
