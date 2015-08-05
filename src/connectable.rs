pub trait Connectable {
    fn get_connection_str(&self) -> String;
}

impl <'a> Connectable for (& 'a str, u64) {
    fn get_connection_str(&self) -> String {
        let mut s = "--SERVER=".to_string();
        s.push_str(self.0);
        s.push(':');
        s.push_str(&self.1.to_string());
        return s;
    }
}

impl <'a> Connectable for Vec<(& 'a str, u64)> {
    fn get_connection_str(&self) -> String {
        let mut s = String::new();
        for i in 0..self.len() {
            s.push_str("--SERVER=");
            s.push_str(self[i].0);
            s.push(':');
            s.push_str(&self[i].1.to_string());
            if i != self.len() - 1 {
                s.push(' ');
            }
        }
        return s;
    }
}

#[test]
fn test_tuple() {
    let param = ("localhost", 2333);
    assert!(param.get_connection_str().as_ref() == "--SERVER=localhost:2333".to_owned());
}

#[test]
fn test_vec() {
    let param_1 = vec!(("localhost", 2333));
    assert!(param_1.get_connection_str().as_ref() == "--SERVER=localhost:2333".to_owned());

    let param_2 = vec!(("localhost", 2333), ("127.0.0.1", 11211));
    let actual = param_2.get_connection_str();
    let wanted = "--SERVER=localhost:2333 --SERVER=127.0.0.1:11211";
    assert!(actual.as_ref() == wanted.to_owned());

    let param_3 = vec!(("localhost", 2333), ("127.0.0.1", 11211), ("dev", 12345));
    let actual = param_3.get_connection_str();
    let wanted = "--SERVER=localhost:2333 --SERVER=127.0.0.1:11211 --SERVER=dev:12345";
    assert!(actual.as_ref() == wanted.to_owned());
}
