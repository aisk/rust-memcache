extern crate memcache;

#[test]
fn test_raw() {
    let client = memcache::connect("localhost", 2333).unwrap();
    client.flush(0).unwrap();

    // set
    client.set_raw("foo", &[0x1u8, 0x2u8, 0x3u8], 0, 42).unwrap();

    // get
    let (value, flags) = client.get_raw("foo").unwrap();
    assert!(value == &[0x1u8, 0x2u8, 0x3u8]);
    assert!(flags == 42);

    // replace exist
    client.replace_raw("foo", &[0x1u8], 0, 1024).unwrap();

    let (value, flags) = client.get_raw("foo").unwrap();
    assert!(value == &[0x1u8]);
    assert!(flags == 1024);

    // replace non exist
    assert!(! client.replace_raw("bar", &[0x1u8], 0, 1024).is_ok());

    // add exist
    assert!(client.add_raw("bar", &[0x1u8], 0, 1024).is_ok());

    // add non exist
    assert!(! client.add_raw("bar", &[0x1u8], 0, 1024).is_ok());
}
