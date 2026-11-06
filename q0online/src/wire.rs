//! Room documents go over the wire and into storage packed: a big value is
//! written as `{"$q0z": "<base64 of its deflated JSON>"}` instead of as
//! itself. Drawings are mostly the same few keys and numbers over and over,
//! so they pack to a tenth of their JSON or less.
//!
//! Readers take either form, so rooms stored before packing still load and
//! editors that send plain JSON are still understood. An editor that wants
//! packed answers says so with [`WIRE_HEADER`]; the server unpacks answers
//! for one that does not ([`unpack_value`]), so older editors keep working.

use std::io::{Read, Write};

use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// The one key of a packed value.
pub const PACKED_KEY: &str = "$q0z";
/// The header an editor sends to get packed answers, with [`WIRE_PACKED`].
pub const WIRE_HEADER: &str = "x-q0online-wire";
pub const WIRE_PACKED: &str = "packed";
/// A value whose JSON is smaller than this is left as it is: packing would
/// not save enough to be worth reading back.
const PACK_FROM: usize = 2048;
/// The most one packed value may unpack to. Past it the value is refused
/// rather than inflated without end.
pub const MAX_UNPACKED_BYTES: u64 = 1 << 30;

fn deflate_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, String> {
    // JSON is written a few bytes at a time; the encoder wants big pieces.
    let encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(4));
    let mut buffered = std::io::BufWriter::with_capacity(1 << 16, encoder);
    serde_json::to_writer(&mut buffered, value).map_err(|e| e.to_string())?;
    buffered
        .into_inner()
        .map_err(|e| e.to_string())?
        .finish()
        .map_err(|e| e.to_string())
}

/// `value` packed: base64 of its deflated JSON.
pub fn pack<T: Serialize + ?Sized>(value: &T) -> Result<String, String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(deflate_json(value)?))
}

fn inflate(text: &str) -> Result<impl Read, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(|e| format!("packed value is not base64: {e}"))?;
    Ok(flate2::read::DeflateDecoder::new(std::io::Cursor::new(bytes)).take(MAX_UNPACKED_BYTES))
}

/// A value packed by [`pack`], read back.
pub fn unpack<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    // Parsing from a slice is several times faster than from a reader.
    let mut json = Vec::new();
    inflate(text)?
        .read_to_end(&mut json)
        .map_err(|e| format!("packed value is damaged: {e}"))?;
    serde_json::from_slice(&json).map_err(|e| format!("packed value is damaged: {e}"))
}

/// The text of a packed value, if `value` is one.
fn packed_text(value: &Value) -> Option<&str> {
    match value {
        Value::Object(map) if map.len() == 1 => map.get(PACKED_KEY)?.as_str(),
        _ => None,
    }
}

/// A value as it came off the wire, packed or plain.
pub fn from_wire<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    match packed_text(&value) {
        Some(text) => unpack(text),
        None => serde_json::from_value(value).map_err(|e| e.to_string()),
    }
}

/// Every packed value in `value` unpacked in place, for a reader that only
/// understands plain JSON. Stops with an error once more than `budget`
/// bytes would come out.
pub fn unpack_value(value: &mut Value, budget: u64) -> Result<(), String> {
    let mut left = budget;
    unpack_within(value, &mut left)
}

fn unpack_within(value: &mut Value, left: &mut u64) -> Result<(), String> {
    if let Some(text) = packed_text(value) {
        let mut json = Vec::new();
        inflate(text)?
            .take(*left + 1)
            .read_to_end(&mut json)
            .map_err(|e| format!("packed value is damaged: {e}"))?;
        if json.len() as u64 > *left {
            return Err("unpacked value is too large".into());
        }
        *left -= json.len() as u64;
        *value =
            serde_json::from_slice(&json).map_err(|e| format!("packed value is damaged: {e}"))?;
    }
    match value {
        Value::Array(items) => items
            .iter_mut()
            .try_for_each(|item| unpack_within(item, left)),
        Value::Object(map) => map
            .values_mut()
            .try_for_each(|item| unpack_within(item, left)),
        _ => Ok(()),
    }
}

/// Write `value` packed, always.
pub struct Packed<'a, T: ?Sized>(pub &'a T);

impl<T: Serialize + ?Sized> Serialize for Packed<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeMap};
        let text = pack(self.0).map_err(S::Error::custom)?;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(PACKED_KEY, &text)?;
        map.end()
    }
}

/// Counts bytes written, keeping none.
struct Count(u64);

impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// How long `value`'s plain JSON is, without building it.
pub fn json_len<T: Serialize + ?Sized>(value: &T) -> Result<u64, String> {
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_err(|e| e.to_string())?;
    Ok(count.0)
}

/// `#[serde(with)]` for a field that is always packed.
pub mod packed {
    use super::*;

    pub fn serialize<T: Serialize, S: Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Packed(value).serialize(serializer)
    }

    pub fn deserialize<'de, T: DeserializeOwned, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        from_wire(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// `#[serde(with)]` for an optional field packed when it is big.
pub mod packed_when_big {
    use super::*;

    pub fn serialize<T: Serialize, S: Serializer>(
        value: &Option<T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            None => serializer.serialize_none(),
            Some(inner) => {
                let size = json_len(inner).map_err(serde::ser::Error::custom)?;
                if size < PACK_FROM as u64 {
                    serializer.serialize_some(inner)
                } else {
                    serializer.serialize_some(&Packed(inner))
                }
            }
        }
    }

    pub fn deserialize<'de, T: DeserializeOwned, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<T>, D::Error> {
        match Option::<Value>::deserialize(deserializer)? {
            None | Some(Value::Null) => Ok(None),
            Some(value) => from_wire(value).map(Some).map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Holder {
        #[serde(with = "packed")]
        big: Vec<f32>,
        #[serde(with = "packed_when_big", default)]
        maybe: Option<Vec<u32>>,
    }

    #[test]
    fn packed_values_read_back_and_plain_ones_still_read() {
        let holder = Holder {
            big: (0..5000).map(|i| i as f32 * 0.25).collect(),
            maybe: Some(vec![1, 2, 3]),
        };
        let json = serde_json::to_value(&holder).unwrap();
        assert!(
            json["big"].get(PACKED_KEY).is_some(),
            "the big one is packed"
        );
        assert!(json["maybe"].is_array(), "a small one is left as it is");
        assert_eq!(
            serde_json::from_value::<Holder>(json.clone()).unwrap(),
            holder
        );
        let plain = serde_json::json!({"big": holder.big, "maybe": [1, 2, 3]});
        assert_eq!(serde_json::from_value::<Holder>(plain).unwrap(), holder);
        assert!(
            serde_json::to_vec(&json).unwrap().len() * 2 < json_len(&holder.big).unwrap() as usize
        );
    }

    #[test]
    fn unpacking_for_an_old_reader_gives_plain_json() {
        let holder = Holder {
            big: (0..5000).map(|i| i as f32).collect(),
            maybe: Some((0..3000).collect()),
        };
        let mut json = serde_json::to_value(&holder).unwrap();
        unpack_value(&mut json, 1 << 20).unwrap();
        assert!(json["big"].is_array() && json["maybe"].is_array());
        let mut json = serde_json::to_value(&holder).unwrap();
        assert!(unpack_value(&mut json, 100).is_err(), "over the budget");
    }
}
