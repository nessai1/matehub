//! Serde helper for emitting `i64` fields as JSON **strings**.
//!
//! # Why
//!
//! Our Snowflake IDs are 63-bit integers that at the current epoch already
//! exceed `Number.MAX_SAFE_INTEGER` (2⁵³ ≈ 9 × 10¹⁵). JavaScript's default
//! `number` type is IEEE-754 double, so `JSON.parse("616572589912883600")`
//! silently rounds. A round-trip loses the last ~7 bits and the id no
//! longer matches anything in the database.
//!
//! Every large-scale API that ships snowflakes to browsers — Discord,
//! Slack, Twitter, GitHub — encodes them as strings for this exact reason.
//!
//! # Usage
//!
//! ```ignore
//! use matehub_common::serde_i64;
//!
//! #[derive(Serialize, Deserialize)]
//! pub struct Channel {
//!     #[serde(with = "serde_i64::as_string")]
//!     pub id: i64,
//!     // i32 / small-bounded integers don't need this.
//!     pub position: i32,
//! }
//! ```
//!
//! For `Option<i64>` use the `option` sub-module:
//! ```ignore
//! #[serde(with = "serde_i64::option_as_string", default)]
//! pub thread_root_id: Option<i64>,
//! ```
//!
//! For `Vec<i64>` use the `vec` sub-module:
//! ```ignore
//! #[serde(with = "serde_i64::vec_as_string")]
//! pub groups: Vec<i64>,
//! ```

pub mod as_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        // Accept both numbers and strings on the way in — some callers
        // (older clients, NATS producers still using native integers) will
        // keep sending raw numbers until they update. Lenient parsing here,
        // strict stringification on the way out.
        StringOrNumber::deserialize(d).and_then(|v| match v {
            StringOrNumber::Str(s) => s.parse().map_err(serde::de::Error::custom),
            StringOrNumber::Num(n) => Ok(n),
        })
    }

    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        Str(String),
        Num(i64),
    }
}

pub mod option_as_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(n) => s.serialize_str(&n.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Inner {
            Str(String),
            Num(i64),
        }
        let opt: Option<Inner> = Option::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(Inner::Num(n)) => Ok(Some(n)),
            Some(Inner::Str(s)) if s.is_empty() => Ok(None),
            Some(Inner::Str(s)) => s.parse().map(Some).map_err(serde::de::Error::custom),
        }
    }
}

pub mod vec_as_string {
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};

    pub fn serialize<S: Serializer>(v: &[i64], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for n in v {
            seq.serialize_element(&n.to_string())?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<i64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Inner {
            Str(String),
            Num(i64),
        }
        let parsed: Vec<Inner> = Vec::deserialize(d)?;
        parsed
            .into_iter()
            .map(|v| match v {
                Inner::Num(n) => Ok(n),
                Inner::Str(s) => s.parse().map_err(serde::de::Error::custom),
            })
            .collect()
    }
}
