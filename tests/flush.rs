extern crate memcache;

#[test]
fn test_flush() {
    let client = memcache::connect("localhost", 2333).unwrap();
    assert!(client.flush(0).is_ok());
}
