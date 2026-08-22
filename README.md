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
  - [x] Meta protocol and a scenario-oriented client (experimental, in the `exp` module)
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

The `memcache::exp` module contains a new client built on memcached's [meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt). It is experimental: the API may change between minor versions, so pin the minor version if you use it:

```ini
[dependencies]
memcache = "0.20"  # >= 0.20.0, < 0.21.0
```

It comes in two layers. `Memcache` (and `AsyncMemcache` behind the `tokio` feature) is organized by caching scenario: one verb per scenario, business values in and out, coordination kept inside. `cache.meta()` exposes the protocol layer underneath, a 1:1 typed mapping of every meta command and option, for whatever the verbs do not cover.

### Scenario client

```rust
use std::time::Duration;
use memcache::exp::{Memcache, Ttl};

let cache = Memcache::connect(["127.0.0.1:11211"])?;

// Object cache: a miss is Ok(None), never an error. Every write names
// its lifetime; Ttl::NEVER for no expiry.
cache.set("user:1", "ann", Ttl::secs(600))?;
let name: Option<String> = cache.get("user:1")?;
cache.delete("user:1")?;

// Get or compute, stampede-safe: one caller per key runs the loader,
// concurrent callers in this process share its result, other processes
// wait briefly for the write-back.
let report: String = cache.fetch("report:q3", Ttl::secs(3600), || build_report())?;

// Refresh ahead of expiry so hot keys never actually expire.
let feed: String = cache.fetch(
    "home:feed",
    Ttl::secs(300).refresh_ahead(Duration::from_secs(30)),
    || build_feed(),
)?;

// Atomic read-modify-write, retried on conflict; versions never show.
let items: u64 = cache.update("cart:42", Ttl::secs(1800), |current| current.unwrap_or(0) + 1)?;

// Counters, fixed window: the ttl applies when the key is created.
let n = cache.incr("rate:1.2.3.4", 1, Ttl::secs(60))?;

// Claim a job exactly once.
if cache.add("job:daily", "1", Ttl::secs(86400))? {
    run_daily_report();
}

// Sessions: read and slide the expiry in one command; replace never
// resurrects a revoked session.
let session: Option<String> = cache.get_touch("session:abc", Ttl::secs(1800))?;
cache.replace("session:abc", "updated", Ttl::secs(1800))?;

// Soft invalidation: readers keep the old value for the grace period
// while fetch elects one of them to recompute.
cache.invalidate("report:q3", Duration::from_secs(60))?;

// Byte streams and an atomic take.
cache.append("events:42", b"login;", Ttl::secs(86400))?;
let events: Option<Vec<u8>> = cache.take("events:42")?;

// Page aggregation: one round trip per server, hits only.
let found: std::collections::HashMap<&str, String> = cache.get_many(["a", "b", "c"])?;
```

Values go through the `Encode` / `Decode` traits on the value type: strings, bytes and integers are built in, `Json<T>` wraps any `serde` type behind the `serde_json` feature, and you can implement the traits for your own types. A value that encodes to zero bytes is rejected (`Error::EmptyValue`): zero-byte items are reserved as lease placeholders, and every read folds them into a miss.

Failure is a policy you choose. By default a cache outage is an error; with `degrade(true)` reads become misses, `fetch` runs its loader locally, blind writes are dropped, and verbs whose answer feeds a business decision (`add`, `replace`, `update`, `take`, `incr`, `decr`) still fail. A request that was written but never answered is `Error::Ambiguous` and never degrades. Absorbed failures reach the `on_error` hook:

```rust
let cache = Memcache::builder()
    .timeout(Duration::from_millis(500))
    .degrade(true)
    .on_error(|event| eprintln!("memcache {:?} in {}: {}", event.kind, event.op, event.error))
    .connect(["cache1:11211", "cache2:11211"])?;
```

The async client has the same verbs behind the `tokio` feature. Its `fetch` takes a future and runs it on a detached task, so a cancelled caller never aborts a shared load, and a refresh returns the current value immediately while the loader recomputes in the background:

```rust
use memcache::exp::{AsyncMemcache, Ttl};

let cache = AsyncMemcache::connect(["127.0.0.1:11211"]).await?;
let report: String = cache
    .fetch("report:q3", Ttl::secs(3600), async move { build_report(db).await })
    .await?;
let (user, hits) = tokio::try_join!(
    cache.get::<String>("user:1"),
    cache.incr("hits", 1, Ttl::secs(60)),
)?;
```

### Protocol layer

`cache.meta()` (or `MetaClient` / `AsyncMetaClient` on their own) maps the meta protocol 1:1: every option of `mg` / `ms` / `md` / `ma` is a builder method, results report miss, CAS mismatch and lease state as values, and batches run in one round trip per server with one result per operation:

```rust
use memcache::exp::{Get, MetaClient, Set, Ttl};

let client = MetaClient::connect("127.0.0.1:11211")?;
client.set("foo", "bar").ttl(Ttl::secs(60)).add().send()?;
let result = client.get("foo").send()?;
assert_eq!(result.value.as_deref(), Some(&b"bar"[..]));

let results = client.run_batch(vec![
    Set::new("a", "1").ttl(Ttl::secs(60)).into(),
    Get::new("b").into(),
])?;
let fetched = client.run_many(["a", "b"].map(Get::new))?;

// Leases, CAS and stale flags are all there for hand-rolled flows:
let read = client.get("hot").lease_ttl(30).refresh_before(10).send()?;
if read.won_lease() {
    client.set("hot", recompute()).ttl(Ttl::secs(300)).compare_cas(read.item.cas.unwrap()).send()?;
}
```

Clients are cheap to clone and share their connection pools; pooling, timeouts and routing (rendezvous hashing by default, or your own `Router`) are set on the builder before connecting.

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
