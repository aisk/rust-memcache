extern crate memcache;

#[test]
fn test_destructor() {
    {
        let _ = memcache::connect(&("localhost", 2333));
    }
}
