//! `usize` that deserializes from an integer or a string such as `64MiB`.
//! Decimal (`KB`, `MB`, `GB`) and binary (`KiB`, `MiB`, `GiB`) suffixes are
//! accepted case-insensitively.

use serde::de::{self, Deserializer, Visitor};

pub fn parse(text: &str) -> Result<usize, String> {
    let s = text.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, suffix) = s.split_at(split);
    if digits.is_empty() {
        return Err(format!("`{text}` is not a byte size"));
    }
    let number: usize = digits
        .parse()
        .map_err(|_| format!("`{text}` is not a byte size"))?;
    let unit: usize = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1_000,
        "m" | "mb" => 1_000_000,
        "g" | "gb" => 1_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        other => return Err(format!("unknown byte-size unit `{other}` in `{text}`")),
    };
    number
        .checked_mul(unit)
        .ok_or_else(|| format!("`{text}` overflows"))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    struct V;
    impl Visitor<'_> for V {
        type Value = usize;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a byte count or a size such as \"64MiB\"")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<usize, E> {
            usize::try_from(v).map_err(|_| E::custom("byte size too large"))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<usize, E> {
            usize::try_from(v).map_err(|_| E::custom("byte size must be positive"))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<usize, E> {
            parse(v).map_err(E::custom)
        }
    }
    d.deserialize_any(V)
}
