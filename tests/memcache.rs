extern crate memcache;

#[test]
fn test_connect() {
    let client = memcache::connect("localhost", 2333).unwrap();
    client.flush();
}
