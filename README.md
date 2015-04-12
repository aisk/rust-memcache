# rust-memcache

Memcached client for rust. Supports **Text Protocol** and multiple servers using Consistent Hash Ring.

Provides two ways of accessing Memcached, via `Connection` for single servers or `Client` for multiple instances. Notice both offer the same API of Memcached commands.

* travis-ci: [![Build Status](https://travis-ci.org/aisk/rust-memcache.svg?branch=master)](https://travis-ci.org/aisk/rust-memcache)
* crates.io: [memcache](https://crates.io/crates/memcache)

## Usage
```rust
// One can connect to a single server
let mut conn = Connection::connect("localhost", 2333).unwrap();

conn.set("foo", b"bar", 0).unwrap();
assert!{ conn.get("foo").unwrap().unwrap().as_slice() == b"bar" };


// Or many at time
let mut nodes: Vec<NodeInfo> = Vec::new();
nodes.push(NodeInfo{host: "localhost", port: 2333});
nodes.push(NodeInfo{host: "localhost", port: 2334});

let mut client = Client::new(nodes, 2).ok().unwrap();

assert!{ client.flush().is_ok() };
assert!{ client.get("foo").ok().unwrap() == None };

assert!{ client.set("foo", b"bar", 0, 10).ok().unwrap() == true };
let result = client.get("foo");

```

# License

MIT
