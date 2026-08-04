extern crate memcache;
extern crate rand;

use memcache::exp::{Delete, Get, GetStatus, Meta, MetaClient, MutationStatus, Set};
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;

const SERVER: &str = "localhost:12345";

fn gen_random_key() -> String {
    Alphanumeric.sample_string(&mut rng(), 10)
}

#[test]
fn exp_set_get_delete() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    client.noop().unwrap();

    let missing = client.get(&*key).send().unwrap();
    assert_eq!(missing.status, GetStatus::Miss);

    let stored = client.set(&*key, "bar").send().unwrap();
    assert!(stored.stored());

    let fetched = client.get(&*key).send().unwrap();
    assert_eq!(fetched.status, GetStatus::Hit);
    assert_eq!(fetched.value.as_deref(), Some(&b"bar"[..]));

    let deleted = client.delete(&*key).send().unwrap();
    assert!(deleted.stored());
    assert_eq!(client.delete(&*key).send().unwrap().status, MutationStatus::NotFound);
    assert_eq!(client.get(&*key).send().unwrap().status, GetStatus::Miss);
}

#[test]
fn exp_store_modes() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    assert_eq!(
        client.set(&*key, "x").replace().send().unwrap().status,
        MutationStatus::NotFound
    );
    assert!(client.set(&*key, "bar").add().send().unwrap().stored());
    assert_eq!(
        client.set(&*key, "baz").add().send().unwrap().status,
        MutationStatus::AlreadyExists
    );
    assert!(client.set(&*key, "rab").replace().send().unwrap().stored());

    assert!(client.set(&*key, "!").append().send().unwrap().stored());
    assert!(client.set(&*key, "?").prepend().send().unwrap().stored());
    let value = client.get(&*key).send().unwrap().value.unwrap();
    assert_eq!(value, b"?rab!".to_vec());
}

#[test]
fn exp_cas_flow() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    let stored = client.set(&*key, "one").return_cas().send().unwrap();
    let cas = stored.cas.unwrap();

    assert!(client.set(&*key, "two").compare_cas(cas).send().unwrap().stored());
    assert_eq!(
        client.set(&*key, "three").compare_cas(cas).send().unwrap().status,
        MutationStatus::CasMismatch
    );
    assert_eq!(client.get(&*key).send().unwrap().value.as_deref(), Some(&b"two"[..]));

    // unless_cas suppresses the value while the CAS still matches.
    let current = client.get(&*key).meta(Meta::NONE.cas()).send().unwrap();
    let unchanged = client.get(&*key).unless_cas(current.item.cas.unwrap()).send().unwrap();
    assert_eq!(unchanged.status, GetStatus::Unchanged);
    assert_eq!(unchanged.value, None);
}

#[test]
fn exp_arithmetic() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    assert_eq!(client.increment(&*key).send().unwrap().status, MutationStatus::NotFound);

    let vivified = client.increment(&*key).initial(40, 60).send().unwrap();
    assert_eq!(vivified.value, Some(40));

    let incremented = client.increment(&*key).delta(2).send().unwrap();
    assert_eq!(incremented.value, Some(42));

    let decremented = client.decrement(&*key).delta(100).send().unwrap();
    assert_eq!(decremented.value, Some(0), "decrement saturates at zero");
}

#[test]
fn exp_item_meta() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    client.set(&*key, "bar").ttl(100).send().unwrap();
    let result = client.get(&*key).meta(Meta::NONE.cas().ttl().size()).send().unwrap();
    assert!(result.item.cas.is_some());
    assert_eq!(result.item.size, Some(3));
    let ttl = result.item.ttl.unwrap();
    assert!(ttl > 0 && ttl <= 100, "unexpected ttl {}", ttl);
}

#[test]
fn exp_binary_key() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = format!("{} with spaces\x01", gen_random_key());

    assert!(client.set(&*key, "bar").send().unwrap().stored());
    let fetched = client.get(&*key).send().unwrap();
    assert_eq!(fetched.value.as_deref(), Some(&b"bar"[..]));
    assert!(client.delete(&*key).send().unwrap().stored());
}

#[test]
fn exp_run_batch() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key_a = gen_random_key();
    let key_b = gen_random_key();

    let results: Vec<_> = client
        .run_batch(vec![
            Set::new(&*key_a, "1").ttl(60).into(),
            Get::new(&*key_a).into(),
            Get::new(&*key_b).into(),
            Delete::new(&*key_b).into(),
        ])
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(results.len(), 4);
    assert!(results[0].as_mutation().unwrap().stored());
    assert_eq!(results[1].as_get().unwrap().value.as_deref(), Some(&b"1"[..]));
    assert_eq!(results[2].as_get().unwrap().status, GetStatus::Miss);
    assert_eq!(results[3].as_mutation().unwrap().status, MutationStatus::NotFound);
}

#[test]
fn exp_lease() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    // First reader vivifies the key and wins the lease.
    let winner = client.get(&*key).lease_ttl(30).send().unwrap();
    assert_eq!(winner.status, GetStatus::Miss);
    assert!(winner.won_lease());
    let cas = winner.item.cas.unwrap();

    // Concurrent readers see the placeholder as pending, not granted.
    let other = client.get(&*key).lease_ttl(30).send().unwrap();
    assert!(!other.won_lease());

    // The winner fulfills the lease; readers then get the real value.
    assert!(client.set(&*key, "fresh").compare_cas(cas).send().unwrap().stored());
    let fetched = client.get(&*key).send().unwrap();
    assert_eq!(fetched.value.as_deref(), Some(&b"fresh"[..]));
}

#[test]
fn exp_typed_values() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    // Numbers are stored as decimal ASCII, so arithmetic works on them.
    client.set(&*key, 41u64).send().unwrap();
    assert_eq!(client.increment(&*key).send().unwrap().value, Some(42));
    let count: Option<u64> = client.get(&*key).send().unwrap().decode().unwrap();
    assert_eq!(count, Some(42));

    client.set(&*key, String::from("text")).send().unwrap();
    let text: Option<String> = client.get(&*key).send().unwrap().decode().unwrap();
    assert_eq!(text.as_deref(), Some("text"));
    assert!(client.get(&*key).send().unwrap().decode::<u64>().is_err());

    let missing: Option<String> = client.get(&*gen_random_key()).send().unwrap().decode().unwrap();
    assert_eq!(missing, None);
}

#[test]
fn exp_multi_server() {
    let client = MetaClient::connect_multiple(["localhost:12345", "localhost:12346"]).unwrap();
    client.noop().unwrap();

    let keys: Vec<String> = (0..20).map(|_| gen_random_key()).collect();
    for key in &keys {
        assert!(client.set(key.as_str(), key.as_str()).send().unwrap().stored());
    }
    for key in &keys {
        let fetched = client.get(key.as_str()).send().unwrap();
        assert_eq!(fetched.value.as_deref(), Some(key.as_bytes()));
    }

    // A batch spanning both servers comes back in input order.
    let results = client
        .run_batch(keys.iter().map(|key| Get::new(key.as_str()).into()))
        .unwrap();
    for (key, result) in keys.iter().zip(&results) {
        let fetched = result.as_ref().unwrap().as_get().unwrap();
        assert_eq!(fetched.value.as_deref(), Some(key.as_bytes()));
    }

    for key in &keys {
        assert!(client.delete(key.as_str()).send().unwrap().stored());
    }
}

#[test]
fn exp_concurrent_clients() {
    let client = MetaClient::connect(SERVER).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let client = client.clone();
            scope.spawn(move || {
                for _ in 0..10 {
                    let key = gen_random_key();
                    assert!(client.set(key.as_str(), "v").send().unwrap().stored());
                    let fetched = client.get(key.as_str()).send().unwrap();
                    assert_eq!(fetched.value.as_deref(), Some(&b"v"[..]));
                    assert!(client.delete(key.as_str()).send().unwrap().stored());
                }
            });
        }
    });
}

#[test]
fn exp_debug() {
    let client = MetaClient::connect(SERVER).unwrap();
    let key = gen_random_key();

    assert!(client.debug(&*key).unwrap().is_none());
    client.set(&*key, "bar").ttl(60).send().unwrap();
    let fields = client.debug(&*key).unwrap().unwrap();
    assert!(fields.contains_key("exp"), "unexpected debug fields: {:?}", fields);
}

#[cfg(feature = "tokio")]
mod async_tests {
    use super::*;
    use memcache::exp::AsyncMetaClient;

    #[tokio::test]
    async fn exp_async_roundtrip() {
        let client = AsyncMetaClient::connect(SERVER).await.unwrap();
        let key = gen_random_key();

        client.noop().await.unwrap();

        assert!(client.set(&*key, "bar").ttl(60).send().await.unwrap().stored());
        let fetched = client.get(&*key).send().await.unwrap();
        assert_eq!(fetched.status, GetStatus::Hit);
        assert_eq!(fetched.value.as_deref(), Some(&b"bar"[..]));

        let results: Vec<_> = client
            .run_batch(vec![Get::new(&*key).into(), Delete::new(&*key).into()])
            .await
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(results[0].as_get().unwrap().value.as_deref(), Some(&b"bar"[..]));
        assert!(results[1].as_mutation().unwrap().stored());

        assert_eq!(client.get(&*key).send().await.unwrap().status, GetStatus::Miss);
    }
}
