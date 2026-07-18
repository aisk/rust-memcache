/*!
Experimental client built on the memcached
[meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt).

Everything in this module is experimental and may change without notice.

For now only the wire layer is implemented, split in two:

- [`MetaCommand`] / [`MetaResponse`]: framing — request assembly and response
  header parsing, including automatic base64 encoding of binary keys.
- `build_*` / `parse_*` and the `*Options` structs: a typed 1:1 mapping of the
  protocol where every option field corresponds to exactly one protocol flag.
  No serialization and no semantic interpretation happens at this level.

Transports are TCP only: [`MetaConnection`] (blocking) and
[`AsyncMetaConnection`] (tokio, behind the `tokio` feature). A high-level
client with serialization, typed results and pipelining will be layered on
top later.

# Example

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

mod connection;
mod meta_api;
mod meta_command;

#[cfg(feature = "tokio")]
mod async_connection;

#[cfg(feature = "tokio")]
pub use async_connection::AsyncMetaConnection;
pub use connection::MetaConnection;
pub use meta_api::{
    ArithmeticMode, ArithmeticOptions, DeleteOptions, GetOptions, MetaCommandResult, SetMode, SetOptions,
    build_arithmetic, build_debug, build_delete, build_get, build_noop, build_set, parse_debug_result,
    parse_meta_result,
};
pub use meta_command::{MAX_KEY_LENGTH, MetaCommand, MetaOp, MetaResponse, ReturnCode};
