use std::sync::Arc;

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::util::SharedStringMap;

/// Huawei data messages use a JSON string, not FCM's JSON object.
#[derive(Debug, Serialize)]
pub struct HuaweiPayload {
    data: SharedStringMap,
    expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HuaweiPayloadSnapshot {
    data: HashMap<String, String>,
    expires_at: i64,
}

impl HuaweiPayload {
    pub fn new(data: impl Into<SharedStringMap>, expires_at: i64) -> Self {
        Self {
            data: data.into(),
            expires_at,
        }
    }

    pub(crate) fn snapshot(&self) -> HuaweiPayloadSnapshot {
        HuaweiPayloadSnapshot {
            data: self.data.as_map().clone(),
            expires_at: self.expires_at,
        }
    }

    pub(crate) fn from_snapshot(snapshot: HuaweiPayloadSnapshot) -> Self {
        Self::new(snapshot.data, snapshot.expires_at)
    }

    pub fn expired(&self, now: i64) -> bool {
        now >= self.expires_at
    }

    fn message(&self, now: i64) -> Result<serde_json::Value, serde_json::Error> {
        let seconds = self
            .expires_at
            .saturating_sub(now)
            .div_euclid(1_000)
            .clamp(0, 2_419_200);
        Ok(serde_json::json!({
            "data": serde_json::to_string(self.data.as_map())?,
            "android": { "urgency": "NORMAL", "ttl": format!("{seconds}s") }
        }))
    }

    /// The documented 4096-byte limit excludes recipient tokens. Count bytes
    /// after the inner JSON has been escaped into the outer message.
    pub fn encoded_message_len_at(&self, now: i64) -> Result<usize, serde_json::Error> {
        serde_json::to_vec(&self.message(now)?).map(|body| body.len())
    }

    pub fn encoded_body_at(&self, token: &str, now: i64) -> Result<Arc<[u8]>, serde_json::Error> {
        let mut message = self.message(now)?;
        message["token"] = serde_json::json!([token]);
        serde_json::to_vec(&serde_json::json!({"validate_only": false, "message": message}))
            .map(Into::into)
    }

    pub fn encoded_body(&self, token: &str) -> Result<Arc<[u8]>, serde_json::Error> {
        self.encoded_body_at(token, chrono::Utc::now().timestamp_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_limit_is_inclusive_at_4096_utf8_bytes() {
        let mut data = HashMap::new();
        data.insert("body".into(), String::new());
        let overhead = HuaweiPayload::new(data.clone(), i64::MAX)
            .encoded_message_len_at(0)
            .unwrap();
        data.insert("body".into(), "a".repeat(4096 - overhead));
        assert_eq!(
            HuaweiPayload::new(data.clone(), i64::MAX)
                .encoded_message_len_at(0)
                .unwrap(),
            4096
        );
        data.get_mut("body").unwrap().push('a');
        assert_eq!(
            HuaweiPayload::new(data, i64::MAX)
                .encoded_message_len_at(0)
                .unwrap(),
            4097
        );
    }

    #[test]
    fn double_encoding_is_counted_and_tokens_are_excluded() {
        let mut data = HashMap::new();
        data.insert("body".into(), "中文\\\"".repeat(80));
        let payload = HuaweiPayload::new(data, 70_000);
        let body = payload.encoded_body_at(&"t".repeat(1024), 10_000).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let inner: HashMap<String, String> =
            serde_json::from_str(value["message"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(inner, *payload.data.as_map());
        assert!(value["message"].get("notification").is_none());
        value["message"].as_object_mut().unwrap().remove("token");
        assert_eq!(
            payload.encoded_message_len_at(10_000).unwrap(),
            serde_json::to_vec(&value["message"]).unwrap().len()
        );
    }

    #[test]
    fn retries_use_remaining_ttl_and_snapshot_keeps_deadline() {
        let payload = HuaweiPayload::new(HashMap::new(), 70_000);
        let restored = HuaweiPayload::from_snapshot(payload.snapshot());
        let body = restored.encoded_body_at("token", 60_000).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["message"]["android"]["ttl"], "10s");
        assert_eq!(value["message"]["android"]["urgency"], "NORMAL");
        assert!(restored.expired(70_000));
    }
}
