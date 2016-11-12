extern crate memcache;

#[test]
fn connect() {
    memcache::connection::connect().unwrap();
}

#[test]
fn flush() {
    let mut conn = memcache::connection::connect().unwrap();
    conn.flush().unwrap();
}

#[test]
fn version() {
    let mut conn = memcache::connection::connect().unwrap();
    conn.version().unwrap();
}

#[test]
fn store() {
    let mut conn = memcache::connection::connect().unwrap();
    conn.set("foo", b"bar", 1, 42).unwrap();
    conn.replace("foo", b"bar", 1, 42).unwrap();
}

#[test]
fn get() {
    let mut conn = memcache::connection::connect().unwrap();
    conn.flush().unwrap();
    conn.set("foo", b"bar", 1, 42).unwrap();
    conn.get(&["foo", "bar", "baz"]).unwrap();
}

#[test]
fn delete() {
    let mut conn = memcache::connection::connect().unwrap();
    conn.delete("foo").unwrap();
}
