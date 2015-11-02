extern crate memcache;

#[test]
fn test_counter() {
    let client = memcache::connect(&("localhost", 2333)).unwrap();
    client.flush(0).unwrap();

    client.set_raw("truth", &[0x30u8], 0, 0).unwrap();
    client.increment("truth", 42).unwrap();
    let (value, flag) = client.get_raw("truth").unwrap();
    assert!(flag == 0);
    assert!(value == &[0x34, 0x32]);
}
