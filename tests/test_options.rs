extern crate memcache;

#[test]
fn test_default() {
    let options: &memcache::Options = &Default::default();
    assert!(options.exptime == 0);
    assert!(options.flags == 0);
    assert!(options.noreply == false);
}

#[test]
fn test_exptime() {
    let mut conn = memcache::Connection::connect("localhost:12345").unwrap();
    conn.set_with_options(
        "key_with_exptime",
        "plus one second",
        &memcache::Options {
            exptime: 1234,
            ..Default::default()
        },
    ).unwrap();
}
