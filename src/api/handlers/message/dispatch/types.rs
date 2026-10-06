use std::sync::Arc;

use hashbrown::HashMap;

use super::*;
use crate::delivery_core::execution::provider::ResolvedProviderTarget as CoreResolvedProviderTarget;

pub(super) struct ResolvedProviderTarget {
    pub(super) channel_type: crate::routing::DeviceChannelType,
    pub(super) device: DeviceInfo,
    pub(super) device_key: Arc<str>,
    pub(super) route_updated_at: i64,
    pub(super) provider_stats_key: Arc<str>,
    pub(super) wakeup_data_for_device: Arc<HashMap<String, String>>,
    pub(super) allow_inline: bool,
    pub(super) provider_pull_delivery: Option<ProviderPullDelivery>,
}

impl ResolvedProviderTarget {
    pub(super) fn provider_name(&self) -> &'static str {
        if self.channel_type == crate::routing::DeviceChannelType::Huawei {
            "HUAWEI"
        } else {
            self.device.platform.provider_name()
        }
    }
}

impl From<CoreResolvedProviderTarget> for ResolvedProviderTarget {
    fn from(value: CoreResolvedProviderTarget) -> Self {
        Self {
            device: value.device,
            channel_type: value.channel_type,
            device_key: value.device_key,
            route_updated_at: value.route_updated_at,
            provider_stats_key: value.provider_stats_key,
            wakeup_data_for_device: value.wakeup_data_for_device,
            allow_inline: value.allow_inline,
            provider_pull_delivery: value.provider_pull_target.map(ProviderPullDelivery::from),
        }
    }
}
