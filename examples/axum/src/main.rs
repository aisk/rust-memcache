//! An example of how to setup Axum with memcached, using a shared
//! connection pool that is accessible to all requests through Axum's
//! state.
//! Run the example with:
//!
//! ```not_rust
//! cargo run -p example-axum
//! ```

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::get,
    Router, Server,
};
use memcache::{self, Pool, Url};

#[derive(Clone)]
struct AppState {
    memcache: Arc<memcache::Client>,
}

async fn get_root(State(app_state): State<AppState>, Path(key): Path<String>) -> String {
    let memcache = app_state.memcache.clone();

    match memcache.get(&key) {
        Ok(Some(value)) => value,
        Ok(None) => "Not found".to_string(),
        Err(err) => format!("ERROR: {}", err),
    }
}

async fn post_root(State(app_state): State<AppState>, Path(key): Path<String>, body: String) -> String {
    let memcache = app_state.memcache.clone();

    match memcache.set(&key, body, 300) {
        Ok(_) => "OK".to_string(),
        Err(e) => format!("ERROR: {}", e),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("This example requires a memcached server running on 127.0.0.1:11211");

    let memcached_url = "memcache://127.0.0.1:11211";
    let memcached_url = Url::parse(memcached_url)?;

    let pool = Pool::builder()
        .max_size(10)
        .build(memcache::ConnectionManager::new(memcached_url))?;
    let client = memcache::Client::with_pool(pool)?;

    let app = Router::new()
        .route("/kv/:key", get(get_root).post(post_root))
        .with_state(AppState {
            memcache: Arc::new(client),
        });

    println!("Starting server on http://0.0.0.0:3000");
    println!("Set keys using [POST] http://0.0.0.0:3000/kv/<key> with a body for the value");
    println!("Get keys using [GET]  http://0.0.0.0:3000/kv/<key>");

    Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
    Ok(())
}
