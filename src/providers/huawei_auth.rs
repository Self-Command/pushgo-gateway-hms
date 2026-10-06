use std::{sync::Arc, time::{Duration, Instant}};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;
use crate::{Error, providers::{BoxFuture, TokenInfo}};

/// Credentials belong to a server-side authorization provider, never a message or APK.
pub trait HuaweiTokenProvider: Send + Sync {
    fn app_id(&self) -> &str;
    fn token_info<'a>(&'a self, fresh: bool) -> BoxFuture<'a, Result<TokenInfo, Error>>;
    fn invalidate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, ()>;
}

pub struct LocalHuaweiTokenProvider {
    client: Client,
    credentials: Option<(String, String)>,
    oauth_url: String,
    access: Mutex<Option<(Arc<str>, Instant)>>,
}

impl LocalHuaweiTokenProvider {
    pub(super) fn new(client: Client, app_id: String, secret: String, oauth_url: String) -> Self {
        Self { client, credentials: Some((app_id, secret)), oauth_url, access: Mutex::new(None) }
    }
    async fn access(&self, fresh: bool) -> Result<TokenInfo, Error> {
        let Some((app_id, secret)) = self.credentials.as_ref() else {
            return Err(Error::Internal("Huawei provider is not configured".into()));
        };
        // Hold the lock through acquisition so concurrent workers share one refresh.
        let mut cache = self.access.lock().await;
        if !fresh
            && let Some((token, deadline)) = cache.as_ref()
            && *deadline > Instant::now()
        {
            return Ok(TokenInfo {
                token: Arc::clone(token),
                expires_in: deadline.saturating_duration_since(Instant::now()).as_secs(),
            });
        }
        let response = self
            .client
            .post(&self.oauth_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", app_id.as_str()),
                ("client_secret", secret.as_str()),
            ])
            .send()
            .await
            .map_err(|_| Error::Internal("Huawei OAuth transport failed".into()))?;
        if !response.status().is_success() {
            return Err(Error::Internal(format!(
                "Huawei OAuth HTTP {}",
                response.status().as_u16()
            )));
        }
        let token: OAuthResponse = response
            .json()
            .await
            .map_err(|_| Error::Internal("Huawei OAuth response invalid".into()))?;
        if token.access_token.is_empty() || token.expires_in == 0 {
            return Err(Error::Internal("Huawei OAuth token missing".into()));
        }
        let token_value: Arc<str> = Arc::from(token.access_token);
        let lifetime = token.expires_in.min(86_400).saturating_sub(60).max(1);
        *cache = Some((
            Arc::clone(&token_value),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(TokenInfo {
            token: token_value,
            expires_in: lifetime,
        })
    }

 }

impl HuaweiTokenProvider for LocalHuaweiTokenProvider {
    fn app_id(&self) -> &str { self.credentials.as_ref().expect("local credentials").0.as_str() }
    fn token_info<'a>(&'a self, fresh: bool) -> BoxFuture<'a, Result<TokenInfo, Error>> { Box::pin(self.access(fresh)) }
    fn invalidate<'a>(&'a self, used: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut cache = self.access.lock().await;
            if cache.as_ref().is_some_and(|(token, _)| token.as_ref() == used) { *cache = None; }
        })
    }
}

#[derive(Deserialize)]
struct OAuthResponse { access_token: String, expires_in: u64 }
