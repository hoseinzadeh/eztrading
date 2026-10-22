use serde::{Deserialize, Serialize};
use serde_json::Result;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Contents {
    pub machines: Vec<Machine>,
}

impl Contents {
    pub fn new() -> Self {
        Self { machines: vec![] }
    }
}

impl Contents {
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json)
    }

    pub fn into_json(&self) -> Result<String> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Machine {
    pub name: String,
    pub params: Vec<Param>,
    
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Param {
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub value: Any,
    #[serde(default)]
    pub options: Options,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Options {
    List(Vec<OptionItem>),
    Range(Range),
    None,
}

impl Default for Options {
    fn default() -> Self {
        Options::None
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionItem {
    pub name: String,
    pub value: Any,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Range {
    pub lo: f32,
    pub hi: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Any {
    pub size: usize,
    pub data: String,
}

impl Any {
    pub fn convert_from<T: Serialize>(obj: &T) -> Self {
        let bytes = bincode::serialize(obj).expect("Could not serialize object");
        let size = bytes.len();
        let mut data = String::new();
        for b in bytes {
            data += &format!("{:02x}", b);
        }
        Self { size, data }
    }

    pub fn convert_to<'a, T: Deserialize<'a>>(&self) -> T {
        self.try_convert_to().expect("Conversion failed")
    }

    pub fn try_convert_to<'a, T: Deserialize<'a>>(&self) -> bincode::Result<T> {
        let mut bytes = vec![];
        let mut s = self.data.clone();
        while !s.is_empty() {
            let (byte, rest) = s.split_at(2);
            bytes.push(u8::from_str_radix(byte, 16).unwrap());
            s = rest.to_owned();
        }
        let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        bincode::deserialize::<T>(slice)
    }
}

mod test {

    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn to_json() {
        let opt = Param {
            id: "name".to_owned(),
            name: "name".to_owned(),
            r#type: "name".to_owned(),
            value: Any::convert_from(&vec![1,2,3,4,5]),
            options: Options::Range(Range {
                lo: 10f32,
                hi: 43f32,
            }),
        };
        println!("{}", serde_json::to_string(&opt).unwrap());
    }
}