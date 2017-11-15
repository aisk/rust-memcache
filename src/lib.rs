/*!
rust-memcache is a [Memcached](https://memcached.org/) client written in pure rust.

# Install:

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "*"
```

# Features:

- <input type="checkbox"  disabled checked /> Binary protocal
- <input type="checkbox"  disabled checked /> TCP connection
- <input type="checkbox"  disabled /> UDP connection
- <input type="checkbox"  disabled checked/> UNIX Domain socket connection
- <input type="checkbox"  disabled /> Automatically compress
- <input type="checkbox"  disabled /> Automatically serialize to JSON / msgpack etc.
- <input type="checkbox"  disabled checked /> Typed interface
- <input type="checkbox"  disabled /> Mutiple server support with custom key hash algorithm

# Basic usage:

```rust
// create connection:
let mut client = memcache::Client::new("memcache://127.0.0.1:12345").unwrap();

// flush the database:
client.flush().unwrap();

// set a string value:
client.set("foo", "bar").unwrap();
// set a key with expiration seconds:
client.set_with_expiration("foo", "bar", 10).unwrap();

// retrieve from memcached
let value: Option<String> = client.get("foo").unwrap();
assert_eq!(value, Some(String::from("bar")));
```
!*/

extern crate byteorder;
extern crate url;

mod connection;
mod error;
mod value;
mod options;
mod packet;
mod client;

pub use connection::Connection;
pub use error::MemcacheError;
pub use options::Options;
pub use value::{ToMemcacheValue, FromMemcacheValue};
pub use client::Client;
