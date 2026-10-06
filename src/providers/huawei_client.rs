use std::sync::Arc;

use reqwest::Client;
use serde::Deserialize;

use crate::{
    Error,
    providers::{
        BoxFuture, DispatchResult, HuaweiClient, PROVIDER_CONNECT_TIMEOUT,
        PROVIDER_REQUEST_TIMEOUT, ProviderFailure, ProviderFailureKind, TokenInfo,
        error::parse_retry_after_millis, huawei::HuaweiPayload,
    },
};

/// Application OAuth credentials stay on the server; deliberately no Debug.
pub struct HuaweiService {
    client: Client,
    authorization: Option<Arc<dyn super::huawei_auth::HuaweiTokenProvider>>,
    push_url: String,
}

impl HuaweiService {
    pub fn disabled() -> Self {
        Self {
            client: Client::new(),
            authorization: None,
            push_url: String::new(),
        }
    }

    pub fn new(app_id: String, secret: String) -> Result<Self, Error> {
        Self::with_endpoints(
            app_id,
            secret,
            "https://oauth-login.cloud.huawei.com/oauth2/v3/token".into(),
            "https://push-api.cloud.huawei.com".into(),
        )
    }

    fn with_endpoints(
        app_id: String,
        secret: String,
        oauth_url: String,
        push_url: String,
    ) -> Result<Self, Error> {
        if app_id.is_empty()
            || !app_id.bytes().all(|b| b.is_ascii_digit())
            || secret.trim().is_empty()
        {
            return Err(Error::Internal(
                "Huawei application credentials missing or invalid".into(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(PROVIDER_CONNECT_TIMEOUT)
            .timeout(PROVIDER_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Internal("Huawei HTTP client initialization failed".into()))?;
        Ok(Self {
            client: client.clone(),
            authorization: Some(Arc::new(super::huawei_auth::LocalHuaweiTokenProvider::new(
                client.clone(),
                app_id,
                secret,
                oauth_url,
            ))),
            push_url,
        })
    }

    async fn access(&self, fresh: bool) -> Result<TokenInfo, Error> {
        self.authorization
            .as_ref()
            .ok_or_else(|| Error::Internal("Huawei provider is not configured".into()))?
            .token_info(fresh)
            .await
    }

    async fn send(&self, token: &str, payload: Arc<HuaweiPayload>) -> DispatchResult {
        let now = chrono::Utc::now().timestamp_millis();
        if payload.expired(now) {
            return DispatchResult::upstream(
                "HUAWEI",
                ProviderFailure::new(0, ProviderFailureKind::Rejected, "Huawei delivery expired"),
            );
        }
        match payload.encoded_message_len_at(now) {
            Ok(size) if size <= 4096 => {}
            Ok(_) => {
                return DispatchResult::upstream(
                    "HUAWEI",
                    ProviderFailure::new(
                        0,
                        ProviderFailureKind::PayloadTooLarge,
                        "Huawei message exceeds 4096 bytes",
                    ),
                );
            }
            Err(_) => {
                return DispatchResult::from_error(
                    0,
                    Error::Internal("Huawei encoding failed".into()),
                );
            }
        }
        let access = match self.access(false).await {
            Ok(token) => token,
            Err(err) => return DispatchResult::provider_access_failure(err),
        };
        if payload.expired(chrono::Utc::now().timestamp_millis()) {
            return DispatchResult::upstream(
                "HUAWEI",
                ProviderFailure::new(
                    0,
                    ProviderFailureKind::Rejected,
                    "Huawei delivery expired during OAuth acquisition",
                ),
            );
        }
        let app_id = self
            .authorization
            .as_ref()
            .expect("authorization acquired")
            .app_id();
        // Re-encode at send time: persisted/prepared bodies must not reset TTL.
        let body = match payload.encoded_body(token) {
            Ok(body) => body,
            Err(_) => {
                return DispatchResult::from_error(
                    0,
                    Error::Internal("Huawei encoding failed".into()),
                );
            }
        };
        let response = match self
            .client
            .post(format!(
                "{}/v1/{app_id}/messages:send",
                self.push_url.trim_end_matches('/')
            ))
            .bearer_auth(access.token.as_ref())
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                return DispatchResult::transport(Error::Internal(
                    "Huawei Push transport failed".into(),
                ));
            }
        };
        let status = response.status().as_u16();
        let retry_after = parse_retry_after_millis(
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        );
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(_) => {
                return DispatchResult::transport(Error::Internal(
                    "Huawei response read failed".into(),
                ));
            }
        };
        let code = serde_json::from_slice::<PushResponse>(&body)
            .ok()
            .map(|r| r.code);
        if (200..300).contains(&status) && code.as_deref() == Some("80000000") {
            return DispatchResult::success(status);
        }
        let failure =
            classify_failure(status, code.as_deref()).with_retry_after_millis(retry_after);
        if failure.kind.should_refresh_credentials()
            && let Some(authorization) = &self.authorization
        {
            authorization.invalidate(access.token.as_ref()).await;
        }
        DispatchResult::upstream("HUAWEI", failure)
    }
}

#[derive(Deserialize)]
struct PushResponse {
    code: String,
}

fn classify_failure(status: u16, code: Option<&str>) -> ProviderFailure {
    let kind = match code {
        Some("80200001" | "80200003") => ProviderFailureKind::CredentialsExpired,
        Some("80300008") => ProviderFailureKind::PayloadTooLarge,
        // 80300007 also covers app/package mismatch; never delete a device route.
        Some("80300002" | "80300007" | "80600003") => ProviderFailureKind::Unauthorized,
        Some("81000001") => ProviderFailureKind::TemporarilyUnavailable,
        _ if matches!(status, 429 | 503) => ProviderFailureKind::RateLimited,
        _ if status == 408 || status >= 500 || code.is_none() => {
            ProviderFailureKind::TemporarilyUnavailable
        }
        _ if status == 401 => ProviderFailureKind::CredentialsExpired,
        _ => ProviderFailureKind::Rejected,
    };
    // Never retain raw msg: Huawei may include recipient tokens in it.
    let safe_code = code
        .filter(|s| s.len() <= 16 && s.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("unknown");
    ProviderFailure::new(
        status,
        kind,
        format!("Huawei HTTP {status}, code {safe_code}"),
    )
}

impl HuaweiClient for HuaweiService {
    fn is_configured(&self) -> bool {
        self.authorization.is_some()
    }
    fn send_to_device<'a>(
        &'a self,
        token: &'a str,
        payload: Arc<HuaweiPayload>,
        _body: Option<Arc<[u8]>>,
    ) -> BoxFuture<'a, DispatchResult> {
        Box::pin(self.send(token, payload))
    }
    fn token_info<'a>(&'a self) -> BoxFuture<'a, Result<TokenInfo, Error>> {
        Box::pin(self.access(false))
    }
    fn token_info_fresh<'a>(&'a self) -> BoxFuture<'a, Result<TokenInfo, Error>> {
        Box::pin(self.access(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct MockState {
        oauth: Arc<AtomicUsize>,
        sends: Arc<AtomicUsize>,
    }
    async fn oauth(State(state): State<MockState>) -> Json<serde_json::Value> {
        state.oauth.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({ "access_token": "test-access", "expires_in": 3600 }))
    }
    async fn push(
        State(state): State<MockState>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        assert_eq!(body["message"]["android"]["urgency"], "NORMAL");
        assert!(body["message"]["data"].is_string());
        assert!(body["message"].get("notification").is_none());
        let attempt = state.sends.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({"code": if attempt == 0 { "80200003" } else { "80000000" }}))
    }

    #[tokio::test]
    async fn oauth_is_singleflight_and_expired_access_is_refreshed_on_durable_retry() {
        let state = MockState {
            oauth: Arc::new(AtomicUsize::new(0)),
            sends: Arc::new(AtomicUsize::new(0)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/oauth", post(oauth))
            .route("/v1/123/messages:send", post(push))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let service = Arc::new(
            HuaweiService::with_endpoints(
                "123".into(),
                "test-secret".into(),
                format!("{base}/oauth"),
                base,
            )
            .unwrap(),
        );
        let mut requests = Vec::new();
        for _ in 0..16 {
            let service = Arc::clone(&service);
            requests.push(tokio::spawn(
                async move { service.access(false).await.unwrap() },
            ));
        }
        for request in requests {
            request.await.unwrap();
        }
        assert_eq!(state.oauth.load(Ordering::SeqCst), 1);
        let payload = Arc::new(HuaweiPayload::new(
            hashbrown::HashMap::new(),
            chrono::Utc::now().timestamp_millis() + 60_000,
        ));
        let first = service.send("test-token", Arc::clone(&payload)).await;
        assert!(!first.success);
        assert!(first.should_refresh_credentials() && first.is_retryable());
        assert!(service.send("test-token", payload).await.success);
        assert_eq!(state.oauth.load(Ordering::SeqCst), 2);
        let expired = Arc::new(HuaweiPayload::new(hashbrown::HashMap::new(), 0));
        assert!(!service.send("test-token", expired).await.success);
        assert_eq!(state.sends.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[test]
    fn http_success_is_not_provider_success_and_configuration_never_deletes_routes() {
        for code in ["80100000", "80300007", "80600003", "80200001"] {
            let failure = classify_failure(200, Some(code));
            assert!(!failure.kind.is_invalid_token());
        }
        assert_eq!(
            classify_failure(200, Some("80300008")).kind,
            ProviderFailureKind::PayloadTooLarge
        );
        assert_eq!(
            classify_failure(503, None).kind,
            ProviderFailureKind::RateLimited
        );
        assert_eq!(
            classify_failure(200, None).kind,
            ProviderFailureKind::TemporarilyUnavailable
        );
        assert!(
            !classify_failure(200, Some("token-sensitive"))
                .message
                .contains("token-sensitive")
        );
    }
}
