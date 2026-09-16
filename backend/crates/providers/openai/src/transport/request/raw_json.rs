use std::{borrow::Cow, fmt};

use bytes::Bytes;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;

use super::{
    ACCOUNT_BOUND_STATE_KEYS, CROSS_ACCOUNT_IDENTITY_KEYS, INSTALLATION_ID_KEYS, TURN_METADATA_KEYS,
};

/// Scope standalone endpoint identities without normalizing opaque business JSON.
/// Invalid/nonobject bodies retain their existing transparent delivery behavior.
pub fn scope_raw_json_to_account(body: &Bytes, installation_id: &str) -> Bytes {
    let Ok(raw) = serde_json::from_slice::<&RawValue>(body) else {
        return body.clone();
    };
    let Ok(Some(scoped)) = scope_object(raw.get(), installation_id, Scope::Root) else {
        return body.clone();
    };
    let start = raw.get().as_ptr() as usize - body.as_ptr() as usize;
    let end = start + raw.get().len();
    let mut output = Vec::with_capacity(start + scoped.len() + body.len() - end);
    output.extend_from_slice(&body[..start]);
    output.extend_from_slice(scoped.as_bytes());
    output.extend_from_slice(&body[end..]);
    Bytes::from(output)
}

pub(crate) fn scope_raw_turn_metadata(raw: &str, installation_id: &str) -> Option<String> {
    let mut scoped = scope_body_turn_metadata(raw, installation_id)?;
    // Validated JSON cannot contain literal CR/LF inside strings. Remove only
    // formatting newlines for HTTP headers, leaving escaped string data intact.
    scoped.retain(|ch| ch != '\r' && ch != '\n');
    Some(scoped)
}

fn scope_body_turn_metadata(raw: &str, installation_id: &str) -> Option<String> {
    let scoped = scope_object(raw, installation_id, Scope::Turn).ok()?;
    let scoped = scoped.as_deref().unwrap_or(raw);
    // JSON permits non-ASCII only inside strings. Escape those characters in the
    // validated raw object, preserving duplicate members and existing escapes.
    if scoped.is_ascii() {
        return Some(scoped.to_owned());
    }
    let mut ascii = String::with_capacity(scoped.len());
    for ch in scoped.chars() {
        if ch.is_ascii() {
            ascii.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                use std::fmt::Write as _;
                write!(&mut ascii, "\\u{unit:04x}").ok()?;
            }
        }
    }
    Some(ascii)
}

#[derive(Clone, Copy)]
enum Scope {
    Root,
    Client,
    Turn,
}

enum Change {
    Keep,
    Remove,
    Replace(String),
}

fn scope_object(
    raw: &str,
    installation_id: &str,
    scope: Scope,
) -> Result<Option<String>, serde_json::Error> {
    let object = serde_json::from_str::<RawObject<'_>>(raw)?;
    let mut cursor = raw.find('{').unwrap_or(0) + 1;
    let opening = cursor;
    let mut changed = false;
    let mut retained = Vec::with_capacity(object.0.len());
    for (index, (key, value)) in object.0.into_iter().enumerate() {
        let start = value.get().as_ptr() as usize - raw.as_ptr() as usize;
        // serde has already parsed member boundaries. Borrow the original key,
        // colon and whitespace instead of re-encoding decoded member names.
        let prefix = &raw[cursor..start];
        let prefix = if index == 0 {
            prefix
        } else {
            &prefix[prefix.find(',').unwrap_or(0) + 1..]
        };
        cursor = start + value.get().len();
        match member_change(&key, value, installation_id, scope) {
            Change::Keep => retained.push((prefix, Cow::Borrowed(value.get()))),
            Change::Remove => changed = true,
            Change::Replace(replacement) => {
                changed = true;
                retained.push((prefix, Cow::Owned(replacement)));
            }
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut output = String::with_capacity(raw.len());
    output.push_str(&raw[..opening]);
    for (index, (prefix, value)) in retained.into_iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(prefix);
        output.push_str(&value);
    }
    output.push_str(&raw[cursor..]);
    Ok(Some(output))
}

fn member_change(key: &str, value: &RawValue, installation_id: &str, scope: Scope) -> Change {
    if CROSS_ACCOUNT_IDENTITY_KEYS.contains(&key) || ACCOUNT_BOUND_STATE_KEYS.contains(&key) {
        return Change::Remove;
    }
    if INSTALLATION_ID_KEYS.contains(&key) {
        if serde_json::from_str::<String>(value.get()).is_ok_and(|id| id == installation_id) {
            return Change::Keep;
        }
        return match serde_json::to_string(installation_id) {
            Ok(replacement) => Change::Replace(replacement),
            Err(_) => Change::Remove,
        };
    }
    if TURN_METADATA_KEYS.contains(&key) {
        if matches!(scope, Scope::Turn) {
            return Change::Remove;
        }
        let Ok(metadata) = serde_json::from_str::<String>(value.get()) else {
            return Change::Keep;
        };
        let Some(scoped) = scope_body_turn_metadata(&metadata, installation_id) else {
            return Change::Keep;
        };
        if metadata == scoped {
            return Change::Keep;
        }
        return match serde_json::to_string(&scoped) {
            Ok(replacement) => Change::Replace(replacement),
            Err(_) => Change::Remove,
        };
    }
    if key == "client_metadata"
        && matches!(scope, Scope::Root)
        && let Ok(Some(scoped)) = scope_object(value.get(), installation_id, Scope::Client)
    {
        return Change::Replace(scoped);
    }
    Change::Keep
}

struct RawObject<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for RawObject<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = RawObject<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry::<String, &'de RawValue>()? {
                    members.push(member);
                }
                Ok(RawObject(members))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}
