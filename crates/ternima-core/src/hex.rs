//! Numbers written as hex text in ternima's files, the way people read them.

/// A u16 as four hex digits, such as "0483", the way USB IDs are usually shown.
pub mod u16 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(value: &u16, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:04x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
        let text = String::deserialize(deserializer)?;
        u16::from_str_radix(text.trim_start_matches("0x"), 16).map_err(D::Error::custom)
    }
}

/// An optional address, such as "0x20000400".
pub mod address {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(address) => serializer.serialize_str(&format!("{address:#x}")),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        let Some(text) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        let digits = text
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        if digits.is_empty() {
            return Ok(None);
        }
        u64::from_str_radix(digits, 16)
            .map(Some)
            .map_err(D::Error::custom)
    }
}
