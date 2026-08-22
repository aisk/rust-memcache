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

TLS support is behind the `tls` feature:

```ini
[dependencies]
memcache = { version = "*", features = ["tls"] }
```

Connect with a `memcache+tls://` URL. Query parameters: `verify_mode` (`peer` by default, or `none`), `ca_path`, and `cert_path` with `key_path`, all PEM files.

## Features

### `memcache::exp` (experimental)

The high-level client that future work focuses on. See [High-level client based on the meta protocol](#high-level-client-based-on-the-meta-protocol-experimental) and the full guide in [docs/exp.md](docs/exp.md).

- [x] Meta protocol
  - [x] High-level verbs: `update` with retry loop, `fetch` with dogpile protection
  - [x] Batches
  - [x] Raw meta commands via `cache.meta()`
- [ ] Connections
  - [x] TCP connection
  - [ ] UNIX Domain socket connection
  - [ ] TLS connection
- [x] Connection pool per server
- [x] Memcached cluster support
  - [x] Rendezvous hashing with custom key hash algorithm
  - [x] Custom placement via the `Router` trait
- [ ] Encodings
  - [x] Typed interface via `Encode` / `Decode`
  - [x] Serialize to JSON via `serde` (`serde_json` feature)
  - [ ] Automatically compress
- [x] Failure policy and error observability hook
- [x] Async client on tokio (`tokio` feature)

### `memcache::Client` (stable)

The classic client. See [Basic usage](#basic-usage).

- [x] All memcached supported protocols
  - [x] Binary protocol
  - [x] ASCII protocol
- [x] All memcached supported connections
  - [x] TCP connection
  - [x] UDP connection
  - [x] UNIX Domain socket connection
  - [x] TLS connection (`tls` feature)
- [x] Typed interface
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

## High-level client based on the meta protocol (Experimental)

> **Experimental.** The high-level client lives under `memcache::exp` and its API
> may change in any minor release. If you depend on it, pin the **minor version**
> in `Cargo.toml`. Patch releases (`x.y.Z`) will not introduce breaking changes,
> but minor releases (`x.Y.0`) might.
>
> ```toml
> [dependencies]
> memcache = "0.21"   # allows 0.21.x, blocks 0.22+
> ```

`memcache::exp::Memcache` hides the [meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt) behind verbs named for what you are doing. CAS tokens and leases never surface in caller code: `update` runs the read, compare and swap, retry loop for you, and `fetch` makes sure a missing value is computed once. The full guide, including the builder options, every verb, batches, the failure policy, the async client and raw protocol access, is in [docs/exp.md](docs/exp.md).

```rust
use memcache::exp::{Json, Memcache, Ttl};

let cache = Memcache::connect(["cache1:11211", "cache2:11211"])?;

// Plain read and write. A miss is `Ok(None)`, never an error.
cache.set("user:1", Json(&user), Ttl::secs(600))?;
let user: Option<Json<User>> = cache.get("user:1")?;

// Get or compute: on a miss one caller across all processes runs the loader,
// everyone else waits for or reuses its result.
let report: Report = cache.fetch("report:q3", Ttl::secs(3600), || build_report())?;

// Atomic read, modify, write with retry on conflict.
let cart: Json<Cart> = cache.update("cart:42", Ttl::secs(1800), |current| {
    let mut cart = current.map(Json::into_inner).unwrap_or_default();
    cart.items.push(item.clone());
    Json(cart)
})?;

// Fixed window rate limiting: the ttl is set at creation and never extended.
if cache.incr(format!("rate:{ip}"), 1, Ttl::secs(60))? > 100 {
    return Err(TooManyRequests);
}
```

Behind the `tokio` feature, `AsyncMemcache` offers the same verbs plus `.await`.

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
