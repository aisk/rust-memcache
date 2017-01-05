extern crate memcache;

#[test]
fn string() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();

    conn.set("this_is_a_string", String::from("a string"), 0).unwrap();
    let s: String = conn.get("this_is_a_string").unwrap();
    assert!(s.as_str() == "a string");

    conn.set("this_is_another_string", "another string", 0).unwrap();
    let s: String = conn.get("this_is_another_string").unwrap();
    assert!(s.as_str() == "another string");
}

#[test]
fn bytes() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();

    conn.set("this_is_a_bytes", "some bytes".as_bytes(), 0).unwrap();
    let b: Vec<u8> = conn.get("this_is_a_bytes").unwrap();
    assert!(b == b"some bytes");
}

#[test]
fn number() {
    let mut conn = memcache::connection::connect("127.0.0.1:12345").unwrap();

    conn.set("this_is_a_u32", 233.to_string(), 0).unwrap();
    let u: u32 = conn.get("this_is_a_u32").unwrap();
    assert!(u == 233);

    conn.set("this_is_a_i32", (-233).to_string(), 0).unwrap();
    let i: i32 = conn.get("this_is_a_i32").unwrap();
    assert!(i == -233);

    conn.set("this_is_a_f64", (233.333).to_string(), 0).unwrap();
    let f: f64 = conn.get("this_is_a_f64").unwrap();
    assert!(f == 233.333);
}
