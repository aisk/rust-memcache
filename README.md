# rust-memcache

Memcached client for rust.

## Usage
```rust
let mut conn = Connection::connect("localhost", 2333).unwrap();

conn.set("foo", b"bar", 0).unwrap();
assert!{ conn.get("foo").unwrap().unwrap().as_slice() == b"bar" };
```

# License

MIT
