extern crate memcache;

#[test]
fn test_connect() {
    let client = memcache::connect("localhost", 2333).unwrap();
    assert!(client.flush(1).is_ok());
}
