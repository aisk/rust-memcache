/*!
Experimental client built on the memcached
[meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt).

Everything in this module is experimental and may change without notice.

The module is layered bottom-up:

- [`MetaCommand`] / [`MetaResponse`]: framing — request assembly and response
  header parsing, including automatic base64 encoding of binary keys.
- `build_*` / `parse_*` and the `*Options` structs: a typed 1:1 mapping of the
  protocol where every option field corresponds to exactly one protocol flag.
  No serialization and no semantic interpretation happens at this level.
- Operations ([`Get`], [`Set`], [`Delete`], [`Arithmetic`]) and typed results
  ([`GetResult`], [`MutationResult`], [`ArithmeticResult`]): the semantic
  layer, interpreting return codes and lease/stale flags. Values are raw
  bytes.
- [`MetaClient`] (blocking) and [`AsyncMetaClient`] (tokio, behind the
  `tokio` feature): single-server clients running one operation at a time.

Transports are TCP only. Serialization, multi-server routing and pipelining
are not implemented yet.

# Example

```no_run
use memcache::exp::{Get, MetaClient, Set};

let mut client = MetaClient::connect("127.0.0.1:11211").unwrap();
client.set(&Set::new("foo", "bar")).unwrap();
let result = client.get(&Get::new("foo")).unwrap();
assert_eq!(result.value.as_deref(), Some(&b"bar"[..]));

// Fields not covered by `new` are set with struct update syntax:
client.set(&Set { ttl: Some(60), ..Set::new("foo", "bar") }).unwrap();
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
mod result;

#[cfg(feature = "tokio")]
mod async_client;
#[cfg(feature = "tokio")]
mod async_connection;

#[cfg(feature = "tokio")]
pub use async_client::AsyncMetaClient;
#[cfg(feature = "tokio")]
pub use async_connection::AsyncMetaConnection;
pub use client::MetaClient;
pub use connection::MetaConnection;
pub use meta_api::{
    ArithmeticMode, ArithmeticOptions, DeleteOptions, GetOptions, MetaCommandResult, SetMode, SetOptions,
    build_arithmetic, build_debug, build_delete, build_get, build_noop, build_set, parse_debug_result,
    parse_meta_result,
};
pub use meta_command::{MAX_KEY_LENGTH, MetaCommand, MetaOp, MetaResponse, ReturnCode};
pub use operation::{Arithmetic, Delete, Get, Meta, Set};
pub use result::{
    ArithmeticResult, GetResult, GetStatus, ItemMeta, LeaseState, MutationResult, MutationStatus, ValueState,
};
