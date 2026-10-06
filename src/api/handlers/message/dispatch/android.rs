use super::*;
use crate::delivery_core::execution::provider::{
    ProviderDispatchContext, ProviderDispatchPayload, enqueue_provider_dispatch,
};

pub(super) async fn dispatch(
    prepared: &PreparedDispatch<'_>,
    target: &ResolvedProviderTarget,
    provider_payload: PreparedProviderPayload,
    progress: &mut DispatchProgress,
) -> Result<(), Error> {
    let (payload, path) = match provider_payload {
        PreparedProviderPayload::Fcm {
            direct_payload,
            direct_body,
            wakeup_payload,
            wakeup_body,
            selection,
        } => {
            let path = selection.initial_path.into();
            (
                ProviderDispatchPayload::Fcm {
                    direct_payload,
                    direct_body,
                    wakeup_payload,
                    wakeup_body,
                    initial_path: path,
                    wakeup_payload_within_limit: selection.wakeup_payload_within_limit,
                },
                path,
            )
        }
        PreparedProviderPayload::Huawei {
            direct_payload,
            direct_body,
            wakeup_payload,
            wakeup_body,
            selection,
        } => {
            let path = selection.initial_path.into();
            (
                ProviderDispatchPayload::Huawei {
                    direct_payload,
                    direct_body,
                    wakeup_payload,
                    wakeup_body,
                    initial_path: path,
                    wakeup_payload_within_limit: selection.wakeup_payload_within_limit,
                },
                path,
            )
        }
        _ => {
            return Err(Error::Internal(
                "prepared payload did not match Android target".into(),
            ));
        }
    };
    match enqueue_provider_dispatch(
        ProviderDispatchContext {
            dispatch: prepared.runtime.dispatch_channels(),
            store: prepared.runtime.storage(),
            channel_id: prepared.channel_id,
            correlation_id: Arc::clone(&prepared.correlation_id),
            delivery_id: Arc::clone(&prepared.delivery_id_ref),
            device_key: Arc::clone(&target.device_key),
            device_token: Arc::from(target.device.token_str()),
            route_updated_at: target.route_updated_at,
            accepted_at: prepared.sent_at,
            acceptance_order: prepared.acceptance_order,
            expires_at: prepared.provider_pull_expires_at(),
            outcome: Arc::clone(&prepared.provider_outcome),
        },
        payload,
    )
    .await
    {
        Ok(()) => {
            record_provider_enqueued(prepared, target, progress, path).await;
        }
        Err(err) => {
            prepared.provider_outcome.record_failure();
            record_provider_enqueue_failed(prepared, target, progress, path, &err).await;
            if !matches!(err, DispatchError::DurableEncoding(_)) {
                return Err(Error::Internal(format!(
                    "Android provider durable materialization failed: {}",
                    super::tracing::dispatch_error_detail(&err)
                )));
            }
        }
    }

    Ok(())
}
