extern crate rand;
extern crate memcache;

use std::thread;
use std::time;
use rand::Rng;

fn gen_random_key() -> String {
    return rand::thread_rng()
        .gen_ascii_chars()
        .take(10)
        .collect::<String>();
}

#[test]
fn test() {
    let mut urls = vec![
        "memcache://localhost:12346",
        "memcache://localhost:12347",
        "memcache://localhost:12348",
        "memcache://localhost:12349",
    ];
    if cfg!(unix) {
        urls.push("memcache:///tmp/memcached2.sock");
    }
    let mut client = memcache::Client::new(urls).unwrap();

    client.version().unwrap();

    client.set("foo", "bar", 0).unwrap();
    client.flush().unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    client.set("foo", "bar", 0).unwrap();
    client.flush_with_delay(3).unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("bar")));
    thread::sleep(time::Duration::from_secs(4));
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    let mut keys: Vec<String> = Vec::new();
    for _ in 0..1000 {
        let key = gen_random_key();
        keys.push(key.clone());
        client.set(key.as_str(), "xxx", 0).unwrap();
    }

    for key in keys {
        let value: String = client.get(key.as_str()).unwrap().unwrap();
        assert_eq!(value, "xxx");
    }
}
