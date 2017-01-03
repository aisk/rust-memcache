extern crate memcache;

#[test]
fn connect() {
    memcache::connection::connect("127.0.0.1:12345").unwrap();
}

#[test]
fn flush() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.flush().unwrap();
}

#[test]
fn version() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.version().unwrap();
}

#[test]
fn store() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    let value = &memcache::value::Raw{bytes: b"bar", flags: 0};
    conn.set("foo", value, 42).unwrap();
    conn.replace("foo", value, 42).unwrap();
    conn.add("foo", value, 42).unwrap();
    conn.append("foo", value, 42).unwrap();
    conn.prepend("foo", value, 42).unwrap();
}

#[test]
fn get() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.flush().unwrap();
    let value = &memcache::value::Raw{bytes: b"bar", flags: 0};
    conn.set("foo", value, 42).unwrap();
    let result: Vec<u8> = conn.get("foo").unwrap();
    assert!(result == b"bar");
}

#[test]
fn delete() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.delete("foo").unwrap();
}

#[test]
fn incr() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    let value = &memcache::value::Raw{bytes: b"100", flags: 0};
    conn.set("foo", value, 0).unwrap();
    assert!(conn.incr("foo", 1).unwrap() == Some(101));
}

#[test]
fn decr() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    let value = &memcache::value::Raw{bytes: b"100", flags: 0};
    conn.set("foo", value, 0).unwrap();
    assert!(conn.decr("foo", 1).unwrap() == Some(99));
}
