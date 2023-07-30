//! An example of how to setup a connection pool for memcached
//! connections.
//! Run the example with:
//!
//! ```not_rust
//! cargo run -p example-pooled
//! ```

use memcache::{Client, ConnectionManager, Pool, Url};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let localhost = Url::parse("memcache://localhost:11211")?;
    let pool = Pool::builder().max_size(10).build(ConnectionManager::new(localhost))?;
    let client = Client::with_pool(pool)?;

    // sets a key to a value, with a 10 second expiration time
    client.set("test", "value", 10)?;

    // gets the value of a key
    if let Some(value) = client.get::<String>("test")? {
        println!("test: {}", value);
    }

    // deletes a key
    client.delete("test")?;
    Ok(())
}
