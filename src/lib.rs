/*!
rust-memcache is a [Memcached](https://memcached.org/) client written in pure rust.

# Install:

The crate is called `memcache` and you can depend on it via cargo:

```ini
[dependencies]
memcache = "*"
```

# Features:

- <input type="checkbox"  disabled checked /> ASCII protocal
- <input type="checkbox"  disabled /> Binary protocal
- <input type="checkbox"  disabled checked /> TCP connection
- <input type="checkbox"  disabled /> UDP connection
- <input type="checkbox"  disabled /> UNIX Domain socket connection
- <input type="checkbox"  disabled /> Automatically compress
- <input type="checkbox"  disabled /> Automatically serialize to JSON / msgpack etc.
- <input type="checkbox"  disabled checked /> Typed interface
- <input type="checkbox"  disabled /> Mutiple server support with custom key hash algorithm

# Basic usage:

```rust
// create connection
let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();

// flush the database
conn.flush().unwrap();

// set a string value
conn.set("foo", "bar").unwrap();
// retrieve from memcached
let value: String = conn.get("foo").unwrap();
assert!(value == "bar");

// set a int value
conn.set("number", 42).unwrap();
// increment it atomic
conn.incr("number", 1).unwrap();
// retrieve it as i32
let value: i32 = conn.get("number").unwrap();
assert!(value == 43);
```
!*/

pub use connection::Connection;
pub use error::MemcacheError;
pub use options::Options;
pub use value::{ToMemcacheValue, FromMemcacheValue};

mod connection;
mod error;
mod value;
mod options;
