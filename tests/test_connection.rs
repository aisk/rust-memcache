extern crate memcache;

#[test]
fn connect() {
    memcache::Connection::connect("127.0.0.1:12345").unwrap();
}

#[test]
fn flush() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    conn.flush().unwrap();
}

#[test]
fn version() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    conn.version().unwrap();
}

#[test]
fn store() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    let value = &memcache::Raw {
        bytes: b"bar",
        flags: 0,
    };
    conn.set("foo", value).unwrap();
    conn.replace("foo", value).unwrap();
    conn.add("foo", value).unwrap();
    conn.append("foo", value).unwrap();
    conn.prepend("foo", value).unwrap();
}

#[test]
fn get() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    conn.flush().unwrap();
    let value = &memcache::Raw {
        bytes: b"bar",
        flags: 0,
    };
    conn.set("foo", value).unwrap();
    let result: (Vec<u8>, u16) = conn.get("foo").unwrap();
    assert!(result.0 == b"bar");
    assert!(result.1 == 0);
}

#[test]
fn delete() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    conn.delete("foo").unwrap();
}

#[test]
fn incr() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    let value = &memcache::Raw {
        bytes: b"100",
        flags: 0,
    };
    conn.set("foo", value).unwrap();
    assert!(conn.incr("foo", 1).unwrap() == Some(101));
}

#[test]
fn decr() {
    let mut conn = memcache::Connection::connect("127.0.0.1:12345").unwrap();
    let value = &memcache::Raw {
        bytes: b"100",
        flags: 0,
    };
    conn.set("foo", value).unwrap();
    assert!(conn.decr("foo", 1).unwrap() == Some(99));
}
