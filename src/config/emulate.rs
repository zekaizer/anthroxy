//! `emulate` as a backend or a `[[models]]` entry writes it.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{Deserializer, Error, MapAccess, Visitor};

/// `emulate = "opencode"`, or `emulate = { preset = "opencode", version =
/// "1.20.0" }` to give a variable of the preset another value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulateConfig {
    pub preset: String,
    /// Values given here, by variable name. Whether the preset declares each
    /// is a rule of `validate`, which knows the presets.
    pub values: BTreeMap<String, String>,
}

/// The table key that names the preset; no variable can have this name.
pub(super) const PRESET_KEY: &str = "preset";

impl<'de> Deserialize<'de> for EmulateConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EmulateVisitor;

        impl<'de> Visitor<'de> for EmulateVisitor {
            type Value = EmulateConfig;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a preset name or a table with `preset`")
            }

            fn visit_str<E: Error>(self, name: &str) -> Result<Self::Value, E> {
                Ok(EmulateConfig {
                    preset: name.to_owned(),
                    values: BTreeMap::new(),
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut preset = None;
                let mut values = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value: String = map.next_value()?;
                    match key == PRESET_KEY {
                        true => preset = Some(value),
                        false => {
                            values.insert(key, value);
                        }
                    }
                }
                Ok(EmulateConfig {
                    preset: preset.ok_or_else(|| A::Error::missing_field(PRESET_KEY))?,
                    values,
                })
            }
        }

        deserializer.deserialize_any(EmulateVisitor)
    }
}
