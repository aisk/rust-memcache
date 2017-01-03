extern crate memcache;

#[test]
fn set_string() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.set("this_is_a_string", String::from("a string"), 0).unwrap();
    conn.set("this_is_another_string", "another string", 0).unwrap();
}

#[test]
fn set_bytes() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.set("this_is_a_bytes", "some bytes".as_bytes(), 0).unwrap();
}

#[test]
fn set_number() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();
    conn.set("this_is_a_number", 1.to_string(), 0).unwrap();
}
