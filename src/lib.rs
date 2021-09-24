/*!
rust-memcache is a [memcached](https://memcached.org/) client written in pure rust.

# Install:

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "*"
```

# Features:

- <input type="checkbox"  disabled checked /> All memcached supported protocols
  - <input type="checkbox"  disabled checked /> Binary protocol
  - <input type="checkbox"  disabled checked /> ASCII protocol
- <input type="checkbox"  disabled checked /> All memcached supported connections
  - <input type="checkbox"  disabled checked /> TCP connection
  - <input type="checkbox"  disabled checked /> UDP connection
  - <input type="checkbox"  disabled checked/> UNIX Domain socket connection
  - <input type="checkbox"  disabled checked/> TLS connection
- <input type="checkbox"  disabled /> Encodings
  - <input type="checkbox"  disabled checked /> Typed interface
  - <input type="checkbox"  disabled /> Automatically compress
  - <input type="checkbox"  disabled /> Automatically serialize to JSON / msgpack etc
- <input type="checkbox"  disabled checked /> Mutiple server support with custom key hash algorithm
- <input type="checkbox"  disabled checked /> Authority
  - <input type="checkbox"  disabled checked /> Binary protocol (plain SASL authority)
  - <input type="checkbox"  disabled checked /> ASCII protocol

# Basic usage:

```rust
// create connection with to memcached server node:
let client = memcache::connect("memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true").unwrap();

// flush the database:
client.flush().unwrap();

// set a string value:
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

// delete value:
client.delete("foo").unwrap();

// using counter:
client.set("counter", 40, 0).unwrap();
client.increment("counter", 2).unwrap();
let answer: i32 = client.get("counter").unwrap().unwrap();
assert_eq!(answer, 42);
```
!*/

#![cfg_attr(feature = "cargo-clippy", allow(clippy::needless_return))]

extern crate byteorder;
extern crate enum_dispatch;
#[cfg(feature = "tls")]
extern crate openssl;
extern crate r2d2;
extern crate rand;
extern crate url;

mod client;
mod connection;
mod error;
mod protocol;
mod stream;
mod value;

pub use crate::client::{Client, Connectable};
pub use crate::connection::ConnectionManager;
pub use crate::error::{ClientError, CommandError, MemcacheError, ServerError};
pub use crate::stream::Stream;
pub use crate::value::{FromMemcacheValue, FromMemcacheValueExt, ToMemcacheValue};
pub use r2d2::Error;
pub use url::{ParseError as UrlParseError, Url};

/// R2D2 connection pool
pub type Pool = r2d2::Pool<connection::ConnectionManager>;

/// Create a memcached client instance and connect to memcached server.
///
/// Example:
///
/// ```rust
/// let client = memcache::connect("memcache://localhost:12345").unwrap();
/// ```
pub fn connect<C: Connectable>(target: C) -> Result<Client, MemcacheError> {
    Client::connect(target)
}
