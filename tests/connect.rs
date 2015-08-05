extern crate memcache;

#[test]
fn test_connect() {
    assert!(memcache::connect(&("localhost", 2333)).is_ok());
}
