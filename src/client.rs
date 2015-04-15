use std::collections::HashMap;
use hash_ring::{NodeInfo, HashRing};

use super::{MemcacheResult, Commands, };
use super::connection::Connection;

// Actual memcached client. Holds all connections and perform appropriate job using
// Consistent Hash Ring to balanced between servers and virtual replicas.
pub struct Client {
    ring : HashRing<NodeInfo>,
    connections : HashMap<String, Connection>
}

impl Client {
    // `replicas` are copies of nodes that point to real servers.
    fn new(nodes: Vec<NodeInfo>, replicas: isize) -> MemcacheResult<Client> {
        let mut new_client = Client {
           ring : HashRing::new(nodes.clone(), replicas),
           connections : HashMap::with_capacity(nodes.len())
        };
        
        // since BufStream can't be clonned. The connections reside in Client. So node lookups are then mapped
        // and commands executed using the right connection regardles if a read or virtual node.
        for n in nodes {
            let conn = try!{Connection::connect(n.host, n.port)};
            new_client.connections.insert(n.to_string(), conn);
        }

        Ok(new_client)
    }
}

impl Client {

    fn get_connection_by_key(&mut self, key :&str) -> &mut Connection {
        let node = self.ring.get_node(key.to_string());
        let conn = self.connections.get_mut(&node.to_string()).expect("Inexistent Connection");
        return conn;
    }

}

impl Commands for Client {
    
    fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool> {
        let conn = self.get_connection_by_key(key);
        conn.set(key, value, exptime, flags)
    }

    fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>> {
        let conn = self.get_connection_by_key(key);
        conn.get(key)
    }
    
    fn flush(&mut self) -> MemcacheResult<()> {
        for (_, conn) in self.connections.iter_mut() {
            match conn.flush() {
                Ok(_) => continue,
                e => return e
            }
        }
        Ok(())
    }

    fn delete(&mut self, key: &str) -> MemcacheResult<bool> {
        let conn = self.get_connection_by_key(key);
        conn.delete(key)
    }

    fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        let conn = self.get_connection_by_key(key);
        conn.incr(key, value)
    }

    fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        let conn = self.get_connection_by_key(key);
        conn.decr(key, value)        
    }

}

#[cfg(test)]
mod test {
    use hash_ring::NodeInfo;
    use Commands;
    use client::Client;

    #[test]
    fn test_client() {
        // test client
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});

        let client = Client::new(nodes, 2);
        assert! { client.is_ok() };


        // test_flush
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});
    
        let mut client = Client::new(nodes, 2).ok().unwrap();
        assert!{ client.flush().is_ok() };
 

        // test_set
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});
    
        let mut client = Client::new(nodes, 2).ok().unwrap();
        assert!{ client.flush().is_ok() };
        assert!{ client.set("fooc", b"bar", 10, 0).ok().unwrap() == true };
    

        // test_get
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});
    
        let mut client = Client::new(nodes, 2).ok().unwrap();
    
        assert!{ client.flush().is_ok() };
        assert!{ client.get("fooc").ok().unwrap() == None };
    
        assert!{ client.set("fooc", b"bar", 0, 10).ok().unwrap() == true };
        let result = client.get("fooc");
        let result_tuple = result.ok().unwrap().unwrap();
        assert!{ result_tuple.0 == b"bar" };
        assert!{ result_tuple.1 == 10 };
    
    
        // test_delete
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});
    
        let mut client = Client::new(nodes, 2).ok().unwrap();
    
        assert!{ client.flush().is_ok() };
        assert!{ client.delete("fooc").ok().unwrap() == false };
    
        // test_incr
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});
    
        let mut client = Client::new(nodes, 2).ok().unwrap();
        assert!{ client.flush().is_ok() };
        let mut result = client.incr("liec", 42);
        assert!{ result.ok().unwrap() == None };
    
        assert!{ client.flush().is_ok() };
        client.set("truthc", b"42", 0, 0).ok().unwrap();
        result = client.incr("truthc", 1);
        assert!{ result.ok().unwrap().unwrap() == 43 };
    
    
        // test_decr
        let mut nodes: Vec<NodeInfo> = Vec::new();
        nodes.push(NodeInfo{host: "localhost", port: 2333});
        nodes.push(NodeInfo{host: "localhost", port: 2334});
        let mut client = Client::new(nodes, 2).ok().unwrap();
        assert!{ client.flush().is_ok() };
    
        let mut result = client.decr("lie", 42);
        assert!{ result.ok().unwrap() == None };
    
        assert!{ client.flush().is_ok() };
        client.set("truthc", b"42", 0, 0).ok().unwrap();
        result = client.decr("truthc", 1);
        assert!{ result.ok().unwrap().unwrap() == 41 };
    }
}
