//! JSON shapes for the few project fields serde cannot write as they are:
//! maps keyed by structs become lists of pairs, and pixel and media bytes
//! become base64 text instead of lists of millions of numbers.

pub mod pairs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;
    use std::hash::Hash;

    pub fn serialize<K, V, S>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        S: Serializer,
    {
        serializer.collect_seq(map.iter())
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        let pairs = Vec::<(K, V)>::deserialize(deserializer)?;
        Ok(pairs.into_iter().collect())
    }
}

pub mod b64 {
    use base64::Engine;
    use serde::de::{Error, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Decode;

        impl<'de> Visitor<'de> for Decode {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a base64 string")
            }

            fn visit_str<E: Error>(self, text: &str) -> Result<Self::Value, E> {
                base64::engine::general_purpose::STANDARD
                    .decode(text.as_bytes())
                    .map_err(E::custom)
            }
        }

        // Decode directly from the input slice or the parser's scratch
        // space. An owned copy of a large encoded bitmap is unnecessary.
        deserializer.deserialize_str(Decode)
    }
}

pub mod b64_arc {
    use serde::{Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S: Serializer>(
        bytes: &Arc<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        super::b64::serialize(bytes, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<Vec<u8>>, D::Error> {
        super::b64::deserialize(deserializer).map(Arc::new)
    }
}
