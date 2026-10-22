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

impl Param {
    /// Set the value from text, encoded the way the generated C++ reads it back
    /// (`float`, `double`, `int`, `bool`, `Vec<Symbol>` as comma-separated text).
    pub fn set_value(&mut self, text: &str) -> std::result::Result<(), String> {
        let ty = self.r#type.replace(' ', "");
        let bytes: Vec<u8> = match ty.as_str() {
            "float" => text.trim().parse::<f32>().map_err(|_| format!("`{text}` is not a number"))?.to_le_bytes().to_vec(),
            "double" => text.trim().parse::<f64>().map_err(|_| format!("`{text}` is not a number"))?.to_le_bytes().to_vec(),
            "int" | "int32_t" | "uint" | "uint32_t" | "unsigned" => {
                text.trim().parse::<i64>().map_err(|_| format!("`{text}` is not an integer"))?.to_le_bytes()[..4].to_vec()
            }
            "int64_t" | "uint64_t" | "long" => text.trim().parse::<i64>().map_err(|_| format!("`{text}` is not an integer"))?.to_le_bytes().to_vec(),
            "bool" => vec![match text.trim() {
                "true" | "1" | "yes" => 1,
                "false" | "0" | "no" => 0,
                _ => return Err(format!("`{text}` is not true or false")),
            }],
            t if t.contains("Symbol") && t.starts_with("Vec") => {
                let list: Vec<&str> = text.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
                if list.iter().any(|s| !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/' | '-'))) {
                    return Err("symbols may contain letters, digits and . : / -".into());
                }
                let joined = list.join(",");
                if joined.len() > 128 {
                    return Err("at most 128 characters of symbols".into());
                }
                joined.into_bytes()
            }
            other => return Err(format!("parameters of type `{other}` cannot be set from text")),
        };
        self.value = Any { size: bytes.len(), data: bytes.iter().map(|b| format!("{b:02x}")).collect() };
        Ok(())
    }

    /// A readable form of the current value, for the types `set_value` understands.
    pub fn value_text(&self) -> Option<String> {
        let bytes: Vec<u8> = (0..self.value.data.len() / 2).filter_map(|i| u8::from_str_radix(&self.value.data[2 * i..2 * i + 2], 16).ok()).take(self.value.size).collect();
        let ty = self.r#type.replace(' ', "");
        Some(match ty.as_str() {
            "float" if bytes.len() >= 4 => format!("{}", f32::from_le_bytes(bytes[..4].try_into().ok()?)),
            "double" if bytes.len() >= 8 => format!("{}", f64::from_le_bytes(bytes[..8].try_into().ok()?)),
            "int" | "int32_t" if bytes.len() >= 4 => format!("{}", i32::from_le_bytes(bytes[..4].try_into().ok()?)),
            "uint" | "uint32_t" | "unsigned" if bytes.len() >= 4 => format!("{}", u32::from_le_bytes(bytes[..4].try_into().ok()?)),
            "bool" if !bytes.is_empty() => (bytes[0] != 0).to_string(),
            t if t.starts_with("Vec") && t.contains("Symbol") => String::from_utf8(bytes).ok()?,
            _ => return None,
        })
    }
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
#[cfg(test)]
mod param_tests {
    use super::*;

    fn param(ty: &str) -> Param {
        Param { id: "x".into(), name: "x".into(), r#type: ty.into(), value: Any { size: 0, data: String::new() }, options: Options::None }
    }

    #[test]
    fn values_round_trip_through_text() {
        for (ty, text) in [("float", "0.25"), ("double", "1.5"), ("int", "-7"), ("bool", "true"), ("Vec<Symbol>", "AAPL,MSFT")] {
            let mut p = param(ty);
            p.set_value(text).unwrap();
            assert_eq!(p.value_text().as_deref(), Some(text), "{ty}");
        }
        let mut p = param("float");
        assert!(p.set_value("abc").is_err());
        assert!(param("Account").set_value("x").is_err());
        assert!(param("Vec<Symbol>").set_value("AA PL; drop").is_err());
    }
}
