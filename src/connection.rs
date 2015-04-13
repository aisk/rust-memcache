use std::convert::From;
use std::io::{BufStream, Error, Write, Read, BufRead};
use std::net::TcpStream;
use hash_ring::{NodeInfo, HashRing};
use std::collections::HashMap;

#[derive(Debug)]
pub enum MemcacheError {
    InternalIoError(Error),
    ServerError
}

impl From<Error> for MemcacheError {
    fn from(err: Error) -> MemcacheError {
        MemcacheError::InternalIoError(err)
    }
}

pub type MemcacheResult<T> = Result<T, MemcacheError>;

// Trait defining Memcache protocol both for multi server connection using `Client` as
// well as single connection using `Connection`.
pub trait Commands {
    fn flush(&mut self) -> MemcacheResult<()>;
    fn delete(&mut self, key: &str) -> MemcacheResult<bool>;
    fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>>;
    fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool>;
    fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>>;
    fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>>;
}

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

impl Commands for Client {
    
    fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool> {
        let node = self.ring.get_node(key.to_string());
        let conn = self.connections.get_mut(&node.to_string()).expect("Inexistent Connection");
        conn.set(key, value, exptime, flags)
    }

    fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>> {
        let node = self.ring.get_node(key.to_string());
        let conn = self.connections.get_mut(&node.to_string()).expect("Inexistent Connection");
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
        let node = self.ring.get_node(key.to_string());
        let conn = self.connections.get_mut(&node.to_string()).expect("Inexistent Connection");
        conn.delete(key)
    }

    fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        let node = self.ring.get_node(key.to_string());
        let conn = self.connections.get_mut(&node.to_string()).expect("Inexistent Connection");
        conn.incr(key, value)
    }

    fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        let node = self.ring.get_node(key.to_string());
        let conn = self.connections.get_mut(&node.to_string()).expect("Inexistent Connection");
        conn.decr(key, value)        
    }

}

pub struct Connection {
    pub host: String,
    pub port: u16,
    stream: BufStream<TcpStream>,
}

impl ToString for Connection {
    fn to_string(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}



impl Commands for Connection {
    fn flush(&mut self) -> MemcacheResult<()> {
        try!{ self.stream.write("flush_all\r\n".as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try!{ self.stream.read_line(&mut line) };
        if line != "OK\r\n" {
            return Err(MemcacheError::ServerError);
        }
        return Ok(());
    }

    fn delete(&mut self, key: &str) -> MemcacheResult<bool> {
        try!{ self.stream.write(format!("delete {}\r\n", key).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "DELETED\r\n" {
            return Ok(true);
        } else if line == "NOT_FOUND\r\n" {
            return Ok(false);
        } else {
            return Err(MemcacheError::ServerError);
        }
    }

    fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>> {
        try!{ self.stream.write(format!("get {}\r\n", key).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "END\r\n" {
            return Ok(None);
        }
        let header: Vec<&str> = line.split(' ').collect();
        if header.len() != 4 || header[0] != "VALUE" || header[1] != key {
            return Err(MemcacheError::ServerError);
        }
        let flags: u16 = match header[2].trim().parse() {
            Ok(flags) => { flags }
            Err(_) => { return Err(MemcacheError::ServerError); }
        };
        let length: usize = match header[3].trim().parse() {
            Ok(length) => { length }
            Err(_) => { return Err(MemcacheError::ServerError); }
        };
        let mut buf : Vec<u8> = Vec::with_capacity(length);
        //buf.resize(length, 0); Safe option to the code bellow
        unsafe {
            buf.set_len(length);
        }
        try!{self.stream.read(&mut buf)};
        return Ok(Some((buf, flags)));
    }

    fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool> {
        try!{ self.stream.write(format!("set {} {} {} {}\r\n", key, flags, exptime, value.len()).as_bytes()) };
        try!{ self.stream.write(value) };
        try!{ self.stream.write("\r\n".as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "STORED\r\n" {
            return Ok(true);
        } else if line == "NOT_STORED\r\n" {
            return Ok(false);
        }
        return Err(MemcacheError::ServerError);
    }

    fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        try!{ self.stream.write(format!("incr {} {:b}\r\n", key, value).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "NOT_FOUND\r\n" {
            return Ok(None);
        }
        let x: &[_] = &['\r', '\n'];
        // let trimed_result = result.trim_right_matches(x);
        let value: isize = match line.trim_right_matches(x).parse() {
            Ok(value) => { value }
            Err(_) => { return Err(MemcacheError::ServerError) }
        };
        return Ok(Some(value));
    }

    fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        try!{ self.stream.write(format!("decr {} {:b}\r\n", key, value).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "NOT_FOUND\r\n" {
            return Ok(None);
        }
        let x: &[_] = &['\r', '\n'];
        // let trimed_result = result.trim_right_matches(x);
        let value: isize = match line.trim_right_matches(x).parse() {
            Ok(value) => { value }
            Err(_) => { return Err(MemcacheError::ServerError) }
        };
        return Ok(Some(value));
    }
}

impl Connection {
    pub fn connect(host: &str, port: u16) -> MemcacheResult<Connection> {
    let stream = try!{ TcpStream::connect((host, port)) };
    return Ok(Connection{
            host: host.to_string(),
            port: port,
            stream: BufStream::new(stream)
        });
    }
}


#[test]
fn test_client() {
    let mut nodes: Vec<NodeInfo> = Vec::new();
    nodes.push(NodeInfo{host: "localhost", port: 2333});
    nodes.push(NodeInfo{host: "localhost", port: 2334});

    let client = Client::new(nodes, 2);
    assert! { client.is_ok() };
}


#[test]
fn test_client_flush() {
    let mut nodes: Vec<NodeInfo> = Vec::new();
    nodes.push(NodeInfo{host: "localhost", port: 2333});
    nodes.push(NodeInfo{host: "localhost", port: 2334});

    let mut client = Client::new(nodes, 2).ok().unwrap();
    assert!{ client.flush().is_ok() };
}

#[test]
fn test_client_set() {
    let mut nodes: Vec<NodeInfo> = Vec::new();
    nodes.push(NodeInfo{host: "localhost", port: 2333});
    nodes.push(NodeInfo{host: "localhost", port: 2334});

    let mut client = Client::new(nodes, 2).ok().unwrap();
    assert!{ client.flush().is_ok() };
    assert!{ client.set("fooc", b"bar", 10, 0).ok().unwrap() == true };
}

#[test]
fn test_client_get() {
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
}


#[test]
fn test_client_delete() {
    let mut nodes: Vec<NodeInfo> = Vec::new();
    nodes.push(NodeInfo{host: "localhost", port: 2333});
    nodes.push(NodeInfo{host: "localhost", port: 2334});

    let mut client = Client::new(nodes, 2).ok().unwrap();

    assert!{ client.flush().is_ok() };
    assert!{ client.delete("fooc").ok().unwrap() == false };
}

#[test]
fn test_client_incr() {
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
}


#[test]
fn test_client_decr() {
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


// Testing single connected servers

#[test]
fn test_connect() {
    assert!{ Connection::connect("localhost", 2333).is_ok() };
}

#[test]
fn test_flush() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
}

#[test]
fn test_set() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    assert!{ conn.set("foo", b"bar", 10, 0).ok().unwrap() == true };
}

#[test]
fn test_get() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    assert!{ conn.get("foo").ok().unwrap() == None };

    assert!{ conn.set("foo", b"bar", 0, 10).ok().unwrap() == true };
    let result = conn.get("foo");
    let result_tuple = result.ok().unwrap().unwrap();
    assert!{ result_tuple.0 == b"bar" };
    assert!{ result_tuple.1 == 10 };
}

#[test]
fn test_delete() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    assert!{ conn.delete("foo").ok().unwrap() == false };
}

#[test]
fn test_incr() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    let mut result = conn.incr("lie", 42);
    assert!{ result.ok().unwrap() == None };

    assert!{ conn.flush().is_ok() };
    conn.set("truth", b"42", 0, 0).ok().unwrap();
    result = conn.incr("truth", 1);
    assert!{ result.ok().unwrap().unwrap() == 43 };
}

#[test]
fn test_decr() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    let mut result = conn.decr("lie", 42);
    assert!{ result.ok().unwrap() == None };

    assert!{ conn.flush().is_ok() };
    conn.set("truth", b"42", 0, 0).ok().unwrap();
    result = conn.decr("truth", 1);
    assert!{ result.ok().unwrap().unwrap() == 41 };
}
