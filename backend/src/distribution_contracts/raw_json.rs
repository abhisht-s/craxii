//! Duplicate-key and lexical validation precede construction of serde values.
use super::{ContractResult, invalid};
use serde_json::{Map, Value};
use std::collections::HashSet;
#[derive(Clone, Copy)]
pub struct Limits {
    pub bytes: usize,
    pub nodes: usize,
    pub depth: usize,
}
pub const LARGE_LIMITS: Limits = Limits {
    bytes: 1_048_576,
    nodes: 32_768,
    depth: 32,
};
pub const CONTROL_LIMITS: Limits = Limits {
    bytes: 16_384,
    nodes: 4096,
    depth: 32,
};
struct Parser<'a> {
    input: &'a str,
    at: usize,
    nodes: usize,
    limits: Limits,
}
impl Parser<'_> {
    fn whitespace(&mut self) {
        while self
            .input
            .as_bytes()
            .get(self.at)
            .is_some_and(|c| matches!(c, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.at += 1;
        }
    }
    fn string(&mut self) -> ContractResult<String> {
        let start = self.at;
        self.at += 1;
        while let Some(&c) = self.input.as_bytes().get(self.at) {
            self.at += 1;
            if c == b'"' {
                return serde_json::from_str(&self.input[start..self.at]).map_err(|_| invalid());
            }
            if c == b'\\' {
                if self.at >= self.input.len() {
                    return Err(invalid());
                }
                self.at += 1;
            } else if c < 32 {
                return Err(invalid());
            }
        }
        Err(invalid())
    }
    fn value(&mut self, depth: usize) -> ContractResult<Value> {
        self.whitespace();
        self.nodes += 1;
        if depth > self.limits.depth || self.nodes > self.limits.nodes {
            return Err(invalid());
        }
        let first = *self.input.as_bytes().get(self.at).ok_or_else(invalid)?;
        match first {
            b'"' => Ok(Value::String(self.string()?)),
            b'{' => {
                self.at += 1;
                self.whitespace();
                let mut map = Map::new();
                let mut keys = HashSet::new();
                if self.input.as_bytes().get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Value::Object(map));
                }
                loop {
                    self.whitespace();
                    if self.input.as_bytes().get(self.at) != Some(&b'"') {
                        return Err(invalid());
                    }
                    let key = self.string()?;
                    if key.is_empty()
                        || !key.bytes().all(|c| (32..=126).contains(&c))
                        || matches!(key.as_str(), "__proto__" | "constructor" | "prototype")
                        || !keys.insert(key.clone())
                    {
                        return Err(invalid());
                    }
                    self.whitespace();
                    if self.input.as_bytes().get(self.at) != Some(&b':') {
                        return Err(invalid());
                    }
                    self.at += 1;
                    map.insert(key, self.value(depth + 1)?);
                    self.whitespace();
                    let end = self.input.as_bytes().get(self.at).copied();
                    self.at += 1;
                    if end == Some(b'}') {
                        return Ok(Value::Object(map));
                    }
                    if end != Some(b',') {
                        return Err(invalid());
                    }
                }
            }
            b'[' => {
                self.at += 1;
                self.whitespace();
                let mut a = Vec::new();
                if self.input.as_bytes().get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Value::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.whitespace();
                    let end = self.input.as_bytes().get(self.at).copied();
                    self.at += 1;
                    if end == Some(b']') {
                        return Ok(Value::Array(a));
                    }
                    if end != Some(b',') {
                        return Err(invalid());
                    }
                }
            }
            b't' | b'f' | b'n' => {
                for (token, v) in [
                    ("true", Value::Bool(true)),
                    ("false", Value::Bool(false)),
                    ("null", Value::Null),
                ] {
                    if self.input[self.at..].starts_with(token) {
                        self.at += token.len();
                        return Ok(v);
                    }
                }
                Err(invalid())
            }
            b'0'..=b'9' => {
                let start = self.at;
                self.at += 1;
                if first != b'0' {
                    while self
                        .input
                        .as_bytes()
                        .get(self.at)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        self.at += 1;
                    }
                }
                let n = self.input[start..self.at]
                    .parse::<u32>()
                    .map_err(|_| invalid())?;
                if n > 2_147_483_647 {
                    return Err(invalid());
                }
                Ok(Value::from(n))
            }
            _ => Err(invalid()),
        }
    }
}
pub fn parse_bounded(bytes: &[u8], limits: Limits) -> ContractResult<Value> {
    if bytes.len() > limits.bytes || bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(invalid());
    }
    let input = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut parser = Parser {
        input,
        at: 0,
        nodes: 0,
        limits,
    };
    let result = parser.value(0)?;
    parser.whitespace();
    if parser.at != input.len() {
        return Err(invalid());
    }
    Ok(result)
}
