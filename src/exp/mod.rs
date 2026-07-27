/*!
Experimental client built on the memcached
[meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt).

Everything in this module is experimental and may change without notice.

Public enums and structs are `#[non_exhaustive]`, so the protocol surface can
grow without breaking callers: construct operations and options through
`new`/`Default` and the builder methods, and give matches a wildcard arm.

The module is layered bottom-up:

- [`MetaCommand`] / [`MetaResponse`]: framing - request assembly and response
  header parsing, including automatic base64 encoding of binary keys.
- `build_*` / `parse_*` and the `*Options` structs: a typed 1:1 mapping of the
  protocol where every option field corresponds to exactly one protocol flag.
  No serialization and no semantic interpretation happens at this level.
- Operations ([`Get`], [`Set`], [`Delete`], [`Arithmetic`]) and typed results
  ([`GetResult`], [`MutationResult`], [`ArithmeticResult`]): the semantic
  layer, interpreting return codes and lease/stale flags. Each operation
  implements [`Operation`] with a typed `Output`. Values are raw bytes.
- [`MetaClient`] (blocking) and [`AsyncMetaClient`] (tokio, behind the
  `tokio` feature): single-server clients whose verbs return lazy
  [`Request`] builders, executed with `send()`.

Several operations can run in one round trip: `run_batch` takes
heterogeneous [`Op`] values and returns [`OpResult`]s. Batched operations
execute independently and in order per server - a batch is not a
transaction.

Clients connected to several servers (`connect_multiple`) route each key by
a pluggable hash function and split batches per server, one round trip
each.

Clients are cheap to clone and shareable across threads or tasks; clones
share per-server pools of idle connections. Key hashing, the idle-pool
cap and timeouts are set on [`MetaClientBuilder`] before connecting and
stay fixed for the client's lifetime, so clones can never route or pool
differently. Checkout never blocks: a busy pool just dials another
connection. A connection that fails mid-exchange is dropped instead of
reused. Connections are dialed lazily; connect and I/O timeouts default
to one second (`None` removes the limit).

Transports are TCP only.

# Example

```no_run
use memcache::exp::MetaClient;

let mut client = MetaClient::connect("127.0.0.1:11211").unwrap();
client.set("foo", "bar").send().unwrap();
let result = client.get("foo").send().unwrap();
assert_eq!(result.value.as_deref(), Some(&b"bar"[..]));

// Options are chained before send():
client.set("foo", "bar").ttl(60).add().send().unwrap();
let counter = client.increment("hits").delta(2).initial(0, 60).send().unwrap();

// Values are encoded via ToValue and decoded by the requested type:
client.set("visits", 41u64).send().unwrap();
let visits: Option<u64> = client.get("visits").send().unwrap().decode().unwrap();

// Several operations in one round trip:
use memcache::exp::{Get, Set};
let results = client
    .run_batch(vec![Set::new("a", "1").ttl(60).into(), Get::new("b").into()])
    .unwrap();
```

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
*/

mod client;
mod connection;
mod core;
mod meta_api;
mod meta_command;
mod operation;
mod request;
mod result;
mod value;

#[cfg(feature = "tokio")]
mod async_client;
#[cfg(feature = "tokio")]
mod async_connection;

#[cfg(feature = "tokio")]
pub use async_client::AsyncMetaClient;
#[cfg(feature = "tokio")]
pub use async_connection::AsyncMetaConnection;
pub use client::{MetaClient, MetaClientBuilder};
pub use connection::MetaConnection;
pub use core::Operation;
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
pub use value::{FLAG_BYTES, FLAG_INT, FLAG_STR, FromValue, ToValue};
