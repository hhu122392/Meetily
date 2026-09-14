use super::types::{MeetingContextProfile, MeetingContextSnapshot};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_value).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonicalize_value(value));
            }
            Value::Object(canonical)
        }
        scalar => scalar,
    }
}

pub fn canonical_json_sha256<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_value(value).expect("serializable meeting context value");
    let canonical = canonicalize_value(json);
    let bytes = serde_json::to_vec(&canonical).expect("canonical JSON is serializable");
    format!("{:x}", Sha256::digest(bytes))
}

pub fn profile_sha256(profile: &MeetingContextProfile) -> String {
    canonical_json_sha256(profile)
}

pub fn snapshot_sha256(snapshot: &MeetingContextSnapshot) -> String {
    let mut hashable = snapshot.clone();
    hashable.context_sha256.clear();
    canonical_json_sha256(&hashable)
}
