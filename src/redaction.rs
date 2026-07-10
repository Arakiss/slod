use serde_json::{Map, Value};

/// Stable replacement used for every structurally sensitive value.
pub const REDACTED: &str = "[REDACTED]";

/// Recursively redact secrets from a JSON value without dropping benign fields.
///
/// Redaction is structural: sensitive field names and sensitive HTTP header
/// names are recognized case-insensitively across common naming conventions.
/// Arbitrary string contents are intentionally not parsed as shell or provider
/// syntax.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(redact).collect()),
        Value::Object(object) => redact_object(object),
        _ => value.clone(),
    }
}

fn redact_object(object: &Map<String, Value>) -> Value {
    let sensitive_header_pair = header_name(object).is_some_and(is_sensitive_key);
    let mut redacted = Map::new();

    for (key, value) in object {
        let must_redact =
            is_sensitive_key(key) || (sensitive_header_pair && is_header_value_field(key));
        redacted.insert(
            key.clone(),
            if must_redact {
                Value::String(REDACTED.to_string())
            } else {
                redact(value)
            },
        );
    }

    Value::Object(redacted)
}

fn header_name(object: &Map<String, Value>) -> Option<&str> {
    object.iter().find_map(|(key, value)| {
        let words = key_words(key);
        let names_header = words.as_slice() == ["name"]
            || words.as_slice() == ["key"]
            || words.as_slice() == ["header"]
            || words.as_slice() == ["header", "name"];
        names_header.then(|| value.as_str()).flatten()
    })
}

fn is_header_value_field(key: &str) -> bool {
    let words = key_words(key);
    words.len() == 1 && matches!(words[0].as_str(), "value" | "values")
}

fn is_sensitive_key(key: &str) -> bool {
    let words = key_words(key);
    let compact = words.concat();
    words.iter().any(|word| {
        matches!(
            word.as_str(),
            "secret" | "token" | "password" | "cookie" | "authorization"
        )
    }) || words
        .windows(2)
        .any(|pair| pair[0] == "api" && pair[1] == "key")
        || [
            "secret",
            "token",
            "password",
            "cookie",
            "authorization",
            "apikey",
        ]
        .iter()
        .any(|suffix| compact.ends_with(suffix))
}

fn key_words(key: &str) -> Vec<String> {
    let chars = key.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();

    for (index, ch) in chars.iter().copied().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            push_word(&mut words, &mut current);
            continue;
        }

        let previous = index.checked_sub(1).and_then(|offset| chars.get(offset));
        let next = chars.get(index + 1);
        let camel_boundary = ch.is_ascii_uppercase()
            && previous.is_some_and(|previous| previous.is_ascii_lowercase())
            || ch.is_ascii_uppercase()
                && previous.is_some_and(|previous| previous.is_ascii_uppercase())
                && next.is_some_and(|next| next.is_ascii_lowercase());
        if camel_boundary {
            push_word(&mut words, &mut current);
        }
        current.push(ch.to_ascii_lowercase());
    }
    push_word(&mut words, &mut current);
    words
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn redacts_nested_secret_fields_and_preserves_benign_values() {
        let input = json!({
            "client_secret": "synthetic-client-secret",
            "accessToken": "synthetic-access-token",
            "PASSWORD": "synthetic-password",
            "cookie": "synthetic-cookie",
            "authorization": "Bearer synthetic-authorization",
            "x-api-key": "synthetic-api-key",
            "apikey": "synthetic-compact-api-key",
            "metrics": {"input_tokens": 120, "duration_ms": 10},
            "items": [{"refresh_token": "synthetic-refresh-token", "name": "kept"}]
        });

        let output = redact(&input);

        assert_eq!(output["client_secret"], REDACTED);
        assert_eq!(output["accessToken"], REDACTED);
        assert_eq!(output["PASSWORD"], REDACTED);
        assert_eq!(output["cookie"], REDACTED);
        assert_eq!(output["authorization"], REDACTED);
        assert_eq!(output["x-api-key"], REDACTED);
        assert_eq!(output["apikey"], REDACTED);
        assert_eq!(output["metrics"]["input_tokens"], 120);
        assert_eq!(output["metrics"]["duration_ms"], 10);
        assert_eq!(output["items"][0]["refresh_token"], REDACTED);
        assert_eq!(output["items"][0]["name"], "kept");
    }

    #[test]
    fn redacts_sensitive_header_values_in_maps_and_name_value_pairs() {
        let input = json!({
            "headers": {
                "Authorization": "Bearer synthetic-header",
                "Cookie": "session=synthetic-cookie",
                "X-API-Key": "synthetic-header-key",
                "Accept": "application/json"
            },
            "header_list": [
                {"name": "Proxy-Authorization", "value": "Basic synthetic-proxy"},
                {"headerName": "Set-Cookie", "values": ["session=synthetic"]},
                {"name": "Content-Type", "value": "application/json"}
            ]
        });

        let output = redact(&input);

        assert_eq!(output["headers"]["Authorization"], REDACTED);
        assert_eq!(output["headers"]["Cookie"], REDACTED);
        assert_eq!(output["headers"]["X-API-Key"], REDACTED);
        assert_eq!(output["headers"]["Accept"], "application/json");
        assert_eq!(output["header_list"][0]["value"], REDACTED);
        assert_eq!(output["header_list"][1]["values"], REDACTED);
        assert_eq!(output["header_list"][2]["value"], "application/json");
    }

    #[test]
    fn redaction_is_deterministic_and_idempotent() {
        let input = json!({
            "z": [{"apiKey": "synthetic"}],
            "a": {"safe": true, "password": "synthetic"}
        });

        let first = redact(&input);
        let second = redact(&input);

        assert_eq!(first, second);
        assert_eq!(redact(&first), first);
    }
}
