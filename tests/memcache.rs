extern crate memcache;

#[test]
fn test_memcache() {
    let client = memcache::connect("localhost", 2333);

}
