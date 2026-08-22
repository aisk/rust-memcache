/*!
Experimental client built on the memcached
[meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt).

Everything in this module is experimental and may change without notice.
Public enums and structs are `#[non_exhaustive]`: construct through `new`
/ `Default` and the builder methods, and give matches a wildcard arm.

# Layers

**High-level layer**: [`Memcache`] and, behind the `tokio` feature,
[`AsyncMemcache`]. One verb per caching pattern, business values in and
out, every coordination mechanism (leases, CAS loops, stale tokens) kept
inside. A miss is `Ok(None)` or an absent map key, never an error;
conditional writes answer with `bool`; `fetch` and `update` consume the
miss entirely. Values go through [`Encode`] / [`Decode`] on the value type
([`Json<T>`] with the `serde_json` feature); counters and byte streams
bypass them. Every write names its lifetime as a [`Ttl`]; `fetch` takes a
[`Freshness`], which is a `Ttl` optionally carrying a refresh-ahead
window. Failures are [`Error`]; what degrades and what does not is a
policy chosen on [`MemcacheBuilder`].

**Protocol layer**: [`MetaClient`] / [`AsyncMetaClient`], reachable from a
high-level client as `cache.meta()`. A typed 1:1 mapping of the protocol:
operations ([`Get`], [`Set`], [`Delete`], [`Arithmetic`]) with one builder
method per protocol flag, results ([`GetResult`], [`MutationResult`],
[`ArithmeticResult`]) that report miss, CAS mismatch and lease state as
values, and batches (`run_batch` / `run_many`) with one result per
operation. Values are raw bytes plus client flags. The wire layer
([`MetaCommand`] / [`MetaResponse`], `build_*` / `parse_*`) stays public
for anything above it does not cover.

**Engine**: per-server connection pools (idle cap, idle age, optional
connection cap, a dead pooled connection redialed once), one deadline per
exchange, routing through a [`Router`] (rendezvous hashing by default),
and failure attribution: a request written but not answered surfaces as
[`Error::Ambiguous`] when it has side effects, so a retry is never a
blind guess. Batches stamp every command with an opaque token and verify
the echoes, so a reordered response poisons the connection instead of
being paired with the wrong command.

# High-level client

```no_run
use std::time::Duration;
use memcache::exp::{Memcache, Ttl};

# fn build_report() -> Result<String, std::io::Error> { Ok(String::new()) }
let cache = Memcache::connect(["127.0.0.1:11211"]).unwrap();

cache.set("user:1", "ann", Ttl::secs(600)).unwrap();
let name: Option<String> = cache.get("user:1").unwrap();

// Get or compute, stampede-safe across threads and processes.
let report: String = cache
    .fetch("report:q3", Ttl::secs(3600).refresh_ahead(Duration::from_secs(60)), build_report)
    .unwrap();

// Atomic read-modify-write, retried on conflict.
let n: u64 = cache.update("cart:42", Ttl::secs(1800), |current| current.unwrap_or(0) + 1).unwrap();

// Soft invalidation: readers keep the old value while one recomputes.
cache.invalidate("report:q3", Duration::from_secs(60)).unwrap();
```

The verb table, with what each returns:

| verb | returns | use |
|---|---|---|
| `get`, `get_and_touch` | `Option<T>` | object cache, sessions |
| `get_many` | `HashMap<K, T>` | page aggregation |
| `fetch` | `T` | expensive computation, stampede protection, smooth expiry |
| `set`, `set_many`, `delete`, `delete_many`, `invalidate`, `touch`, `append`, `prepend` | `()` | writes and invalidation |
| `add`, `replace` | `bool` | claim once, never resurrect |
| `update`, `try_update` | `T` | concurrent modification |
| `take` | `Option<T>` | atomic take and delete |
| `incr`, `decr` | `u64` | counters, rate limits |
| `inspect` | `Option<ItemInfo>` | diagnostics |

Zero-byte values are reserved as lease placeholders: a value that encodes
to nothing is rejected with [`Error::EmptyValue`], and reads fold a
zero-byte item into a miss.

# Protocol layer

```no_run
use memcache::exp::{Get, MetaClient, Set, Ttl};

let client = MetaClient::connect("127.0.0.1:11211").unwrap();
client.set("foo", "bar").ttl(Ttl::secs(60)).send().unwrap();
let result = client.get("foo").send().unwrap();
assert_eq!(result.value.as_deref(), Some(&b"bar"[..]));

// Options are chained before send():
client.set("foo", "bar").ttl(Ttl::secs(60)).add().send().unwrap();
let counter = client.increment("hits").delta(2).initial(0, 60).send().unwrap();

// Several operations in one round trip; each gets its own result:
let results = client
    .run_batch(vec![Set::new("a", "1").ttl(Ttl::secs(60)).into(), Get::new("b").into()])
    .unwrap();
assert!(results[0].is_ok());

// Batches of one operation kind keep their types (here: the multiget):
let fetched = client.run_many(["a", "b"].map(Get::new)).unwrap();
assert!(fetched[0].as_ref().unwrap().hit());
```

A lease read (`lease_ttl`, optionally `refresh_before`) makes exactly one
client recompute a missing or expiring value while the others keep
serving the old one; it fetches the item CAS automatically so the refill
can be CAS-guarded:

```no_run
use memcache::exp::{MetaClient, Ttl};

let client = MetaClient::connect("127.0.0.1:11211").unwrap();
let result = client.get("report").lease_ttl(30).refresh_before(10).send().unwrap();
if result.won_lease() {
    let fresh = String::from("recomputed here");
    client
        .set("report", &fresh)
        .ttl(Ttl::secs(300))
        .compare_cas(result.item.cas.unwrap())
        .send()
        .unwrap();
} else if let Some(old) = &result.value {
    // A hit, possibly stale while another client refreshes: serve it.
    let _ = old;
}
```

The legal combinations of `status`, `value_state` and `lease_state` are
listed on [`GetResult`]. TTLs are [`Ttl`] values everywhere: a zero TTL is
a usage error rather than "never expires", a relative TTL above 30 days is
sent as an absolute timestamp, and the `*_raw(u32)` variants pass a wire
value through untouched.

The wire layer remains available for anything the clients do not cover:

```no_run
use memcache::exp::{build_get, parse_meta_result, GetOptions, MetaConnection};

let mut connection = MetaConnection::connect("127.0.0.1:11211").unwrap();
let command = build_get("foo", &GetOptions::default()).unwrap();
let response = connection.execute(&command).unwrap();
let result = parse_meta_result(response).unwrap();
if result.ok() {
    println!("value: {:?}", result.value);
}
```

Transports are TCP only.
*/

mod client;
mod connection;
mod core;
mod error;
mod memcache;
mod meta_api;
mod meta_command;
mod operation;
mod request;
mod result;
mod router;
mod ttl;
mod value;

#[cfg(feature = "tokio")]
mod async_client;
#[cfg(feature = "tokio")]
mod async_connection;
#[cfg(feature = "tokio")]
mod async_memcache;

#[cfg(feature = "tokio")]
pub use async_client::AsyncMetaClient;
#[cfg(feature = "tokio")]
pub use async_connection::AsyncMetaConnection;
#[cfg(feature = "tokio")]
pub use async_memcache::AsyncMemcache;
pub use client::{MetaClient, MetaClientBuilder};
pub use connection::MetaConnection;
pub use core::Operation;
pub use core::verbs::ItemInfo;
pub use error::{Error, Result};
pub use memcache::{ErrorEvent, ErrorKind, Memcache, MemcacheBuilder};
pub use meta_api::{
    ArithmeticMode, ArithmeticOptions, DeleteOptions, GetOptions, MetaCommandResult, SetMode, SetOptions,
    build_arithmetic, build_debug, build_delete, build_get, build_noop, build_set, parse_debug_result,
    parse_meta_result,
};
pub use meta_command::{MAX_KEY_LENGTH, MetaCommand, MetaOp, MetaResponse, ReturnCode};
pub use operation::{Arithmetic, Delete, Get, Meta, Op, Set};
pub use request::Request;
pub use result::{
    ArithmeticResult, GetResult, GetStatus, ItemMeta, LeaseState, MutationResult, MutationStatus, OpResult, ValueState,
};
pub use router::{Rendezvous, Router, ServerAddress, default_hash_function};
pub use ttl::{Freshness, Ttl};
#[cfg(feature = "serde_json")]
pub use value::Json;
pub use value::{Decode, DecodeError, Encode, EncodeError, Encoded, FLAG_BYTES, FLAG_INT, FLAG_JSON, FLAG_STR};
