use super::*;

#[tokio::test]
async fn unconfigured_huawei_does_not_replace_existing_fcm_route() {
    let mut state = build_test_state().await;
    state.huawei_configured = false;
    let app = super::super::build_router(state.clone(), "docs");
    let (_, registered) = post_json(
        app.clone(),
        "/device/register",
        json!({"platform":"android"}),
    )
    .await;
    let key = response_string_field(&registered, "device_key");
    let (status, _) = post_json(
        app.clone(),
        "/channel/device",
        json!({
        "device_key":key,"platform":"android","channel_type":"fcm","provider_token":"fcm-provider-token-unconfigured-test"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_json(app, "/channel/device", json!({
        "device_key":key,"platform":"android","channel_type":"huawei","provider_token":"hms-token"
    })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.to_string().contains("huawei_provider_not_configured"));
    assert_eq!(
        state
            .device_registry
            .get(key)
            .unwrap()
            .provider_token
            .as_deref(),
        Some("fcm-provider-token-unconfigured-test")
    );
}

#[tokio::test]
async fn late_fcm_cleanup_cannot_retire_current_huawei_token() {
    let state = build_test_state().await;
    let app = super::super::build_router(state.clone(), "docs");
    let (_, registered) = post_json(
        app.clone(),
        "/device/register",
        json!({"platform":"android"}),
    )
    .await;
    let key = response_string_field(&registered, "device_key");
    let (status, _) = post_json(app.clone(), "/channel/device", json!({
        "device_key":key,"platform":"android","channel_type":"huawei","provider_token":"same-provider-token"
    })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_json(app, "/channel/device/provider-token/retire", json!({
        "device_key":key,"platform":"android","channel_type":"fcm","provider_token":"same-provider-token"
    })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["retired"], false);
    assert_eq!(
        state
            .device_registry
            .get(key)
            .unwrap()
            .provider_token
            .as_deref(),
        Some("same-provider-token")
    );
}
