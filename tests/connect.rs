extern crate memcache;

#[test]
fn test_connect() {
    assert!(memcache::connect(&("localhost", 2333)).is_ok());
    assert!(memcache::connect(&vec![("localhost", 2333), ("localhost", 2334)]).is_ok());
}
