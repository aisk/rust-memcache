extern crate memcache;

#[test]
fn it_works() {
    memcache::connection::connect().unwrap();
}
