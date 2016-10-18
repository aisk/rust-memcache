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
