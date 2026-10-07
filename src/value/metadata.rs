use serde_json::{Map as JsonMap, Value};

use super::{ValueError, ValueResult};

pub(crate) struct MetadataEntries<'a>(&'a JsonMap<String, Value>);

impl<'a> MetadataEntries<'a> {
    pub(crate) fn new(metadata: &'a JsonMap<String, Value>) -> Self {
        Self(metadata)
    }

    pub(crate) fn parse_value(raw: Value) -> Result<JsonMap<String, Value>, String> {
        match raw {
            Value::Null => Ok(JsonMap::new()),
            Value::Object(object) => parse_metadata_object(object),
            _ => Err("metadata must be a JSON object".to_string()),
        }
    }

    pub(crate) fn validate(&self) -> ValueResult<()> {
        self.validate_with_limit(crate::runtime_config::metadata_max_value_bytes())
    }

    fn validate_with_limit(&self, max_value_bytes: usize) -> ValueResult<()> {
        let mut dedupe = std::collections::HashSet::new();
        for (raw_key, raw_value) in self.0 {
            let key = raw_key.trim();
            if key.is_empty() {
                return Err(ValueError::new("metadata key must not be empty"));
            }
            if key.len() > 64 {
                return Err(ValueError::new("metadata key is too long"));
            }
            if !dedupe.insert(key.to_string()) {
                return Err(ValueError::new("metadata key must be unique"));
            }

            let value = metadata_scalar_text(raw_value)
                .ok_or_else(|| ValueError::new("metadata value must be scalar"))?;
            if value.is_empty() {
                return Err(ValueError::new("metadata value must not be empty"));
            }
            if value.len() > max_value_bytes {
                return Err(ValueError::new(format!(
                    "metadata value is too long (maximum {max_value_bytes} UTF-8 bytes)"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> ValueResult<String> {
        serde_json::to_string(self.0).map_err(|_| ValueError::new("metadata format is invalid"))
    }
}

#[cfg(test)]
pub(crate) struct ExtensionObjectRef<'a> {
    object: &'a JsonMap<String, Value>,
    field: &'a str,
}

#[cfg(test)]
impl<'a> ExtensionObjectRef<'a> {
    pub(crate) fn new(object: &'a JsonMap<String, Value>, field: &'a str) -> Self {
        Self { object, field }
    }

    pub(crate) fn validate(&self) -> ValueResult<()> {
        for (key, value) in self.object {
            if key.trim().is_empty() {
                return Err(ValueError::new(format!(
                    "{} contains empty key",
                    self.field
                )));
            }
            match value {
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
                Value::Object(inner) => {
                    for inner_value in inner.values() {
                        match inner_value {
                            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
                            _ => {
                                return Err(ValueError::new(format!(
                                    "{} only supports one-level objects",
                                    self.field
                                )));
                            }
                        }
                    }
                }
                Value::Array(_) => {
                    return Err(ValueError::new(format!(
                        "{} does not support arrays",
                        self.field
                    )));
                }
            }
        }
        Ok(())
    }
}

fn parse_metadata_object(object: JsonMap<String, Value>) -> Result<JsonMap<String, Value>, String> {
    let mut out = JsonMap::new();
    for (raw_key, raw_value) in object {
        let key = raw_key.trim();
        if key.is_empty() {
            return Err("metadata key must not be empty".to_string());
        }
        if metadata_scalar_text(&raw_value).is_none() {
            return Err(format!("metadata.{key} must be a scalar"));
        }
        if out.insert(key.to_string(), raw_value).is_some() {
            return Err("metadata key must be unique".to_string());
        }
    }
    Ok(out)
}

fn metadata_scalar_text(raw: &Value) -> Option<String> {
    match raw {
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then_some(trimmed.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_rejects_nested_values() {
        let metadata = JsonMap::from_iter([("nested".to_string(), json!({"a": 1}))]);
        let err = MetadataEntries::new(&metadata)
            .validate()
            .expect_err("nested metadata should be rejected");
        assert!(err.to_string().contains("metadata value must be scalar"));
    }

    #[test]
    fn metadata_limit_counts_utf8_bytes_and_preserves_default_boundary() {
        let at_limit = JsonMap::from_iter([("card".to_string(), json!("测".repeat(170) + "ab"))]);
        let over_limit = JsonMap::from_iter([("card".to_string(), json!("测".repeat(171)))]);
        MetadataEntries::new(&at_limit)
            .validate_with_limit(crate::runtime_config::DEFAULT_METADATA_MAX_VALUE_BYTES)
            .expect("exactly 512 UTF-8 bytes should be accepted");
        let error = MetadataEntries::new(&over_limit)
            .validate_with_limit(crate::runtime_config::DEFAULT_METADATA_MAX_VALUE_BYTES)
            .expect_err("513 UTF-8 bytes should exceed the default limit");
        assert!(error.to_string().contains("maximum 512 UTF-8 bytes"));
    }

    #[test]
    fn larger_configured_limit_preserves_complete_card_and_enforces_its_boundary() {
        let raw = json!({"title": "模板任务", "content": "完整任务说明".repeat(80)}).to_string();
        assert!(raw.len() > 1536);
        let metadata = JsonMap::from_iter([("task_card".to_string(), json!(raw))]);
        let entries = MetadataEntries::new(&metadata);
        entries
            .validate_with_limit(8192)
            .expect("a complete template card should fit the configured limit");
        let encoded: Value = serde_json::from_str(&entries.encode().unwrap()).unwrap();
        assert_eq!(encoded["task_card"], metadata["task_card"]);
        let at_limit = JsonMap::from_iter([("card".to_string(), json!("x".repeat(8192)))]);
        MetadataEntries::new(&at_limit)
            .validate_with_limit(8192)
            .expect("exactly 8192 bytes should be accepted");
        let over_limit = JsonMap::from_iter([("card".to_string(), json!("x".repeat(8193)))]);
        assert!(
            MetadataEntries::new(&over_limit)
                .validate_with_limit(8192)
                .is_err()
        );
    }

    #[test]
    fn extension_object_rejects_arrays() {
        let attrs = JsonMap::from_iter([("bad".to_string(), json!(["x"]))]);
        let err = ExtensionObjectRef::new(&attrs, "attrs")
            .validate()
            .expect_err("arrays should be rejected");
        assert!(err.to_string().contains("attrs does not support arrays"));
    }
}
