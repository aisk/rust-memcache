extern crate memcache;

#[test]
fn test_destructor() {
    {
        let client = memcache::connect("localhost", 2333);
    }
}
