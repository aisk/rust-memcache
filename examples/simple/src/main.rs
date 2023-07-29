//! A simple example of using the memcache crate, which
//! connects to a memcached server and sets, gets, and
//! deletes a key.
//! Run the example with:
//!
//! ```not_rust
//! cargo run -p example-simple
//! ```

use memcache::connect;

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let localhost = "memcache://localhost:11211";
    let client = connect(localhost).expect("Couldn't connect to memcached");

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
