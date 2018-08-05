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
- <input type="checkbox"  disabled checked /> UDP connection
- <input type="checkbox"  disabled checked/> UNIX Domain socket connection
- <input type="checkbox"  disabled /> Automatically compress
- <input type="checkbox"  disabled /> Automatically serialize to JSON / msgpack etc.
- <input type="checkbox"  disabled checked /> Typed interface
- <input type="checkbox"  disabled checkbox /> Mutiple server support with custom key hash algorithm

# Basic usage:

```rust
// create connection with to memcached server node:
let mut client = memcache::Client::new("memcache://127.0.0.1:12345").unwrap();

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

extern crate byteorder;
extern crate url;
extern crate rand;

mod connection;
mod udp_stream;
mod error;
mod value;
mod packet;
mod client;

pub use error::MemcacheError;
pub use value::{ToMemcacheValue, FromMemcacheValue};
pub use client::{Client, Connectable};
