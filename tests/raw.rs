extern crate memcache;

#[test]
fn test_raw() {
    let client = memcache::connect("localhost", 2333).unwrap();
    client.set_raw("foo", &[0x1u8, 0x2u8, 0x3u8], 0, 42).unwrap();

    let (value, flags) = client.get_raw("foo").unwrap();
    println!("values: {:?}", value);
    assert!(value == &[0x1i8, 0x2i8, 0x3i8]);
    assert!(flags == 42);
}
