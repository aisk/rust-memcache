extern crate memcache;

#[test]
fn test_raw() {
    let client = memcache::connect("localhost", 2333).unwrap();
    client.flush(0);

    client.set_raw("foo", &[0x1u8, 0x2u8, 0x3u8], 0, 42).unwrap();

    let (value, flags) = client.get_raw("foo").unwrap();
    assert!(value == &[0x1u8, 0x2u8, 0x3u8]);
    assert!(flags == 42);
}
