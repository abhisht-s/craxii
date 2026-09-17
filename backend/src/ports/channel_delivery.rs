//! Outbound-only provider-neutral delivery adapter boundary.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::application::delivery_planner::ChannelDeliveryProfileRegistry;
use crate::domain::{ChannelDispatchResult, ChannelProviderId, PreparedChannelDispatch};

pub type ChannelDeliveryFuture<'a> =
    Pin<Box<dyn Future<Output = ChannelDispatchResult> + Send + 'a>>;

pub trait ChannelDeliveryAdapter: Send + Sync {
    fn provider_id(&self) -> &ChannelProviderId;

    fn dispatch(&self, dispatch: PreparedChannelDispatch) -> ChannelDeliveryFuture<'_>;
}

#[derive(Clone, Default)]
pub struct ChannelDeliveryAdapterRegistry {
    adapters: BTreeMap<ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>>,
}

impl ChannelDeliveryAdapterRegistry {
    pub fn try_new(
        profiles: &ChannelDeliveryProfileRegistry,
        adapters: impl IntoIterator<Item = (ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>)>,
    ) -> Result<Self, AdapterRegistryError> {
        let mut registered = BTreeMap::new();
        for (key, adapter) in adapters {
            if adapter.provider_id() != &key {
                return Err(AdapterRegistryError::ProviderMismatch);
            }
            if !profiles.contains(&key) {
                return Err(AdapterRegistryError::MissingProfile);
            }
            if registered.insert(key, adapter).is_some() {
                return Err(AdapterRegistryError::DuplicateProvider);
            }
        }
        Ok(Self {
            adapters: registered,
        })
    }

    #[must_use]
    pub fn adapter(
        &self,
        provider_id: &ChannelProviderId,
    ) -> Option<&Arc<dyn ChannelDeliveryAdapter>> {
        self.adapters.get(provider_id)
    }
}

impl fmt::Debug for ChannelDeliveryAdapterRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelDeliveryAdapterRegistry")
            .field("adapter_count", &self.adapters.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterRegistryError {
    DuplicateProvider,
    ProviderMismatch,
    MissingProfile,
}

impl fmt::Display for AdapterRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateProvider => "duplicate channel delivery adapter",
            Self::ProviderMismatch => "channel delivery adapter provider mismatch",
            Self::MissingProfile => "channel delivery adapter has no profile",
        })
    }
}

impl std::error::Error for AdapterRegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChannelDeliveryProfile, PreparedChannelDispatch};

    struct FakeAdapter(ChannelProviderId);

    impl ChannelDeliveryAdapter for FakeAdapter {
        fn provider_id(&self) -> &ChannelProviderId {
            &self.0
        }

        fn dispatch(&self, _dispatch: PreparedChannelDispatch) -> ChannelDeliveryFuture<'_> {
            Box::pin(async {
                ChannelDispatchResult::Accepted {
                    external_message_id: None,
                }
            })
        }
    }

    #[test]
    fn registry_enforces_provider_keys_and_profiles_but_profiles_may_stand_alone() {
        let provider = ChannelProviderId::try_new("fake.one").unwrap();
        let other = ChannelProviderId::try_new("fake.two").unwrap();
        let profiles = ChannelDeliveryProfileRegistry::try_new([ChannelDeliveryProfile::try_new(
            provider.clone(),
            64,
            1,
        )
        .unwrap()])
        .unwrap();
        assert!(
            ChannelDeliveryAdapterRegistry::try_new(
                &profiles,
                std::iter::empty::<(ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>)>(),
            )
            .is_ok()
        );
        let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeAdapter(provider.clone()));
        assert_eq!(
            ChannelDeliveryAdapterRegistry::try_new(
                &profiles,
                [(other.clone(), Arc::clone(&adapter))],
            )
            .unwrap_err(),
            AdapterRegistryError::ProviderMismatch
        );
        let other_adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeAdapter(other.clone()));
        assert_eq!(
            ChannelDeliveryAdapterRegistry::try_new(&profiles, [(other, other_adapter)],)
                .unwrap_err(),
            AdapterRegistryError::MissingProfile
        );
        assert_eq!(
            ChannelDeliveryAdapterRegistry::try_new(
                &profiles,
                [
                    (provider.clone(), Arc::clone(&adapter)),
                    (provider, adapter),
                ],
            )
            .unwrap_err(),
            AdapterRegistryError::DuplicateProvider
        );
    }
}
