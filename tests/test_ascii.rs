extern crate memcache;

use std::collections::HashMap;

#[test]
fn test_ascii() {
    let mut client = memcache::Client::connect("memcache://localhost:12345?protocol=ascii").unwrap();

    client.set("ascii_foo", "bar", 0).unwrap();
    let value: Option<String> = client.get("ascii_foo").unwrap();
    assert_eq!(value, Some("bar".into()));

    client.set("ascii_baz", "qux", 0).unwrap();
    let values: HashMap<String, String> = client.gets(vec!["ascii_foo", "ascii_baz", "not_exists_key"]).unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values.get("ascii_foo"), Some(&"bar".to_string()));
    assert_eq!(values.get("ascii_baz"), Some(&"qux".to_string()));

    let value: Option<String> = client.get("not_exists_key").unwrap();
    assert_eq!(value, None);

    client.set("ascii_pend", "y", 0).unwrap();
    client.append("ascii_pend", "z").unwrap();
    let value: Option<String> = client.get("ascii_pend").unwrap();
    assert_eq!(value, Some("yz".into()));
    client.prepend("ascii_pend", "x").unwrap();
    let value: Option<String> = client.get("ascii_pend").unwrap();
    assert_eq!(value, Some("xyz".into()));
}
