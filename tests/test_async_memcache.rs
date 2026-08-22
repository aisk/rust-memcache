//! Integration tests of the tokio high-level client against a live memcached
//! on localhost:12345.

#![cfg(feature = "tokio")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use memcache::exp::{AsyncMemcache, Error, ErrorKind, GetStatus, Ttl};
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;

const SERVER: &str = "localhost:12345";

fn random_key() -> String {
    Alphanumeric.sample_string(&mut rng(), 10)
}

async fn cache() -> AsyncMemcache {
    AsyncMemcache::connect([SERVER]).await.unwrap()
}

#[tokio::test]
async fn verbs_roundtrip() {
    let cache = cache().await;
    let key = random_key();

    assert_eq!(cache.get::<String>(&key).await.unwrap(), None);
    cache.set(&key, "hello", Ttl::secs(60)).await.unwrap();
    assert_eq!(cache.get::<String>(&key).await.unwrap().as_deref(), Some("hello"));
    assert_eq!(
        cache
            .get_and_touch::<String>(&key, Ttl::secs(500))
            .await
            .unwrap()
            .as_deref(),
        Some("hello")
    );
    let info = cache.inspect(&key).await.unwrap().unwrap();
    assert!(info.ttl.unwrap() > Duration::from_secs(400));
    cache.touch(&key, Ttl::NEVER).await.unwrap();
    assert_eq!(cache.inspect(&key).await.unwrap().unwrap().ttl, None);

    assert!(!cache.add(&key, "x", Ttl::secs(60)).await.unwrap());
    assert!(cache.replace(&key, "world", Ttl::secs(60)).await.unwrap());
    let n: String = cache
        .update(&key, Ttl::secs(60), |_: Option<String>| String::from("1"))
        .await
        .unwrap();
    assert_eq!(n, "1");
    let n: u64 = cache
        .try_update(&key, Ttl::secs(60), |current: Option<u64>| async move {
            Ok::<_, Error>(current.unwrap() + 1)
        })
        .await
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(cache.incr(&key, 3, Ttl::secs(60)).await.unwrap(), 5);
    assert_eq!(cache.decr(&key, 10, Ttl::secs(60)).await.unwrap(), 0);
    assert_eq!(cache.take::<u64>(&key).await.unwrap(), Some(0));
    assert_eq!(cache.take::<u64>(&key).await.unwrap(), None);

    cache.append(&key, b"a", Ttl::secs(60)).await.unwrap();
    cache.prepend(&key, b"b", Ttl::secs(60)).await.unwrap();
    assert_eq!(cache.get::<Vec<u8>>(&key).await.unwrap(), Some(b"ba".to_vec()));
    cache.invalidate(&key, Duration::from_secs(60)).await.unwrap();
    assert_eq!(cache.take::<Vec<u8>>(&key).await.unwrap(), None);
    cache.delete(&key).await.unwrap();
    assert_eq!(cache.get::<Vec<u8>>(&key).await.unwrap(), None);

    let keys: Vec<String> = (0..4).map(|_| random_key()).collect();
    cache
        .set_many(keys.iter().take(2).map(|k| (k.as_str(), k.as_str())), Ttl::secs(60))
        .await
        .unwrap();
    let found: HashMap<&str, String> = cache.get_many(keys.iter().map(String::as_str)).await.unwrap();
    assert_eq!(found.len(), 2);
    cache.delete_many(&keys).await.unwrap();
    let found: HashMap<&String, String> = cache.get_many(&keys).await.unwrap();
    assert!(found.is_empty());

    assert!(matches!(
        cache.set(&key, "", Ttl::secs(60)).await,
        Err(Error::EmptyValue)
    ));
    assert!(matches!(cache.set(&key, "x", Ttl::secs(0)).await, Err(Error::Usage(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_and_try_join() {
    let cache = cache().await;
    let key = random_key();
    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let cache = cache.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let mut applied = 0;
                while applied < 10 {
                    match cache
                        .update::<u64, _>(&key, Ttl::secs(60), |c| c.unwrap_or(0) + 1)
                        .await
                    {
                        Ok(_) => applied += 1,
                        Err(Error::Conflict) => {}
                        Err(error) => panic!("{error}"),
                    }
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(cache.get::<u64>(&key).await.unwrap(), Some(40));

    let other = random_key();
    let (value, count, ()) = tokio::try_join!(
        cache.get::<u64>(&key),
        cache.incr(&other, 1, Ttl::secs(60)),
        cache.touch(&key, Ttl::secs(60)),
    )
    .unwrap();
    assert_eq!((value, count), (Some(40), 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_singleflight_survives_a_cancelled_caller() {
    let cache = cache().await;
    let key = random_key();
    let calls = Arc::new(AtomicUsize::new(0));
    let load = |calls: Arc<AtomicUsize>| async move {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok::<_, Error>(String::from("shared"))
    };

    // The first caller takes the lease, then gets dropped mid-wait.
    let first = tokio::spawn({
        let cache = cache.clone();
        let key = key.clone();
        let calls = Arc::clone(&calls);
        async move { cache.fetch(&key, Ttl::secs(60), load(calls)).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    first.abort();

    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let cache = cache.clone();
            let key = key.clone();
            let calls = Arc::clone(&calls);
            tokio::spawn(async move { cache.fetch(&key, Ttl::secs(60), load(calls)).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap(), "shared");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.get::<String>(&key).await.unwrap().as_deref(), Some("shared"));
}

#[tokio::test]
async fn fetch_loader_error_is_shared_and_releases_the_lease() {
    let cache = cache().await;
    let key = random_key();
    let error = cache
        .fetch::<String, _, _>(&key, Ttl::secs(60), async { Err(std::io::Error::other("db down")) })
        .await
        .unwrap_err();
    assert!(error.callback::<std::io::Error>().is_some(), "{error:?}");
    let next = cache.meta().get(&key).lease_ttl(30).send().await.unwrap();
    assert_eq!(next.status, GetStatus::Miss);
    assert!(next.won_lease());
}

#[tokio::test]
async fn fetch_stale_grace_refreshes_in_the_background() {
    let cache = cache().await;
    let key = random_key();
    cache.set(&key, "old", Ttl::secs(60)).await.unwrap();
    cache.invalidate(&key, Duration::from_secs(60)).await.unwrap();

    // The elected reader gets the old value at once.
    let served = cache
        .fetch(&key, Ttl::secs(60), async { Ok::<_, Error>(String::from("new")) })
        .await
        .unwrap();
    assert_eq!(served, "old");
    // The background task writes the new value back.
    for _ in 0..50 {
        if cache.get::<String>(&key).await.unwrap().as_deref() == Some("new") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let fresh = cache.meta().get(&key).send().await.unwrap();
    assert_eq!(fresh.value.as_deref(), Some(&b"new"[..]));
    assert!(!fresh.is_stale());
}

#[tokio::test]
async fn fetch_refresh_ahead_in_the_background() {
    let cache = cache().await;
    let key = random_key();
    let calls = Arc::new(AtomicUsize::new(0));
    let load = |calls: Arc<AtomicUsize>| async move {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, Error>(String::from("v"))
    };
    let freshness = Ttl::secs(100).refresh_ahead(Duration::from_secs(30));
    cache.fetch(&key, freshness, load(calls.clone())).await.unwrap();
    cache.fetch(&key, freshness, load(calls.clone())).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    cache.touch(&key, Ttl::secs(10)).await.unwrap();
    cache.fetch(&key, freshness, load(calls.clone())).await.unwrap();
    for _ in 0..50 {
        if cache.inspect(&key).await.unwrap().unwrap().ttl.unwrap() > Duration::from_secs(60) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(cache.inspect(&key).await.unwrap().unwrap().ttl.unwrap() > Duration::from_secs(60));
}

#[tokio::test]
async fn fetch_background_failures_reach_on_error() {
    let key = random_key();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let cache = AsyncMemcache::builder()
        .on_error(move |event| sink.lock().unwrap().push((event.kind, event.op)))
        .connect_async([SERVER])
        .await
        .unwrap();
    cache.set(&key, "old", Ttl::secs(60)).await.unwrap();
    cache.invalidate(&key, Duration::from_secs(60)).await.unwrap();
    let served = cache
        .fetch::<String, _, _>(&key, Ttl::secs(60), async { Err(std::io::Error::other("db down")) })
        .await
        .unwrap();
    assert_eq!(served, "old");
    for _ in 0..50 {
        if !events.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(events.lock().unwrap()[0], (ErrorKind::BackgroundLoader, "fetch"));
}

#[tokio::test]
async fn fetch_waits_for_another_process_then_computes_locally() {
    let cache = cache().await;
    let key = random_key();
    let other = cache.meta().get(&key).lease_ttl(30).send().await.unwrap();
    assert!(other.won_lease());
    let start = std::time::Instant::now();
    let value = cache
        .fetch(&key, Ttl::secs(60), async { Ok::<_, Error>(String::from("local")) })
        .await
        .unwrap();
    assert_eq!(value, "local");
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(700) && elapsed < Duration::from_secs(3),
        "{elapsed:?}"
    );
    assert_eq!(cache.get::<String>(&key).await.unwrap(), None);
}

#[tokio::test]
async fn degrade_folds_outages_into_misses() {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let cache = AsyncMemcache::builder()
        .degrade(true)
        .connect_timeout(Duration::from_millis(200))
        .on_error(move |event| sink.lock().unwrap().push(event.kind))
        .connect_async([addr])
        .await
        .unwrap();
    assert_eq!(cache.get::<String>("k").await.unwrap(), None);
    assert!(cache.get_many::<String, _>(["a"]).await.unwrap().is_empty());
    cache.set("k", "v", Ttl::secs(5)).await.unwrap();
    cache.delete_many(["k"]).await.unwrap();
    assert_eq!(
        cache
            .fetch("k", Ttl::secs(5), async { Ok::<_, Error>(String::from("local")) })
            .await
            .unwrap(),
        "local"
    );
    assert!(cache.add("k", "v", Ttl::secs(5)).await.is_err());
    assert!(cache.incr("k", 1, Ttl::secs(5)).await.is_err());
    assert!(events.lock().unwrap().iter().all(|kind| *kind == ErrorKind::Degraded));
    assert_eq!(events.lock().unwrap().len(), 5);
}
