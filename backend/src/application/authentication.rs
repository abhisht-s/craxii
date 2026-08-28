//! Application-layer device authentication independent of transport and SQLite.

use std::fmt;

use crate::domain::{AuthenticatedDevice, BearerToken, UtcTimestamp, device_token_hashes_equal};
use crate::ports::device_credentials::DeviceCredentialStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticationErrorKind {
    AuthenticationFailed,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AuthenticationError;

impl AuthenticationError {
    #[must_use]
    pub const fn kind(self) -> AuthenticationErrorKind {
        AuthenticationErrorKind::AuthenticationFailed
    }

    #[must_use]
    pub const fn code(self) -> &'static str {
        "authentication_failed"
    }
}

impl fmt::Display for AuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl fmt::Debug for AuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for AuthenticationError {}

pub struct DeviceAuthenticator<'a, S> {
    store: &'a S,
    #[cfg(test)]
    digest_comparison_observer: Option<&'a dyn Fn()>,
}

impl<'a, S> DeviceAuthenticator<'a, S>
where
    S: DeviceCredentialStore,
{
    #[must_use]
    pub const fn new(store: &'a S) -> Self {
        Self {
            store,
            #[cfg(test)]
            digest_comparison_observer: None,
        }
    }

    #[cfg(test)]
    fn with_digest_comparison_observer(store: &'a S, observer: &'a dyn Fn()) -> Self {
        Self {
            store,
            digest_comparison_observer: Some(observer),
        }
    }

    fn digest_matches(
        &self,
        expected: crate::domain::DeviceTokenHash,
        actual: crate::domain::DeviceTokenHash,
    ) -> bool {
        #[cfg(test)]
        if let Some(observer) = self.digest_comparison_observer {
            observer();
        }
        device_token_hashes_equal(expected, actual)
    }

    /// Validates untrusted edge text and deliberately collapses grammar and lookup failures.
    pub async fn authenticate_bearer(
        &self,
        bearer_text: String,
        observed_at: UtcTimestamp,
    ) -> Result<AuthenticatedDevice, AuthenticationError> {
        let token = BearerToken::parse(bearer_text).map_err(|_| AuthenticationError)?;
        self.authenticate(token, observed_at).await
    }

    pub async fn authenticate(
        &self,
        token: BearerToken,
        observed_at: UtcTimestamp,
    ) -> Result<AuthenticatedDevice, AuthenticationError> {
        let expected_hash = token.token_hash();
        let matched = self
            .store
            .lookup_device_by_token_hash(expected_hash)
            .await
            .map_err(|_| AuthenticationError)?
            .ok_or(AuthenticationError)?;
        let digest_matches = self.digest_matches(expected_hash, matched.matched_hash);
        let revoked = matched.revoked_at.is_some();
        if !digest_matches || revoked {
            return Err(AuthenticationError);
        }

        let authenticated = AuthenticatedDevice::new(matched.device_id);
        let _evidence_only = self
            .store
            .best_effort_touch_last_seen(authenticated.device_id(), observed_at)
            .await;
        Ok(authenticated)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Mutex;

    use super::*;
    use crate::domain::{DeviceId, DeviceTokenHash};
    use crate::ports::device_credentials::{
        DeviceCredentialFuture, DeviceCredentialMatch, DeviceCredentialStoreError,
        DeviceCredentialStoreErrorKind, DeviceSummary, ProvisionDeviceIntent, RevokeDeviceOutcome,
    };

    struct FakeStore {
        matched: Option<DeviceCredentialMatch>,
        fail_lookup: bool,
        fail_touch: bool,
        lookups: Mutex<usize>,
        touches: Mutex<Vec<(DeviceId, UtcTimestamp)>>,
    }

    impl DeviceCredentialStore for FakeStore {
        fn provision_device(
            &self,
            _intent: ProvisionDeviceIntent,
        ) -> DeviceCredentialFuture<'_, DeviceSummary> {
            Box::pin(async { unreachable!() })
        }

        fn lookup_device_by_token_hash(
            &self,
            _token_hash: DeviceTokenHash,
        ) -> DeviceCredentialFuture<'_, Option<DeviceCredentialMatch>> {
            *self.lookups.lock().unwrap() += 1;
            Box::pin(async move {
                if self.fail_lookup {
                    Err(DeviceCredentialStoreError::new(
                        DeviceCredentialStoreErrorKind::Storage,
                    ))
                } else {
                    Ok(self.matched)
                }
            })
        }

        fn list_devices(&self) -> DeviceCredentialFuture<'_, Vec<DeviceSummary>> {
            Box::pin(async { unreachable!() })
        }

        fn revoke_device(
            &self,
            _device_id: DeviceId,
            _revoked_at: UtcTimestamp,
        ) -> DeviceCredentialFuture<'_, RevokeDeviceOutcome> {
            Box::pin(async { unreachable!() })
        }

        fn best_effort_touch_last_seen(
            &self,
            device_id: DeviceId,
            observed_at: UtcTimestamp,
        ) -> DeviceCredentialFuture<'_, ()> {
            Box::pin(async move {
                self.touches.lock().unwrap().push((device_id, observed_at));
                if self.fail_touch {
                    Err(DeviceCredentialStoreError::new(
                        DeviceCredentialStoreErrorKind::Storage,
                    ))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn now() -> UtcTimestamp {
        "2026-08-28T12:34:56.000001Z".parse().unwrap()
    }

    fn store_for(token: &BearerToken) -> FakeStore {
        FakeStore {
            matched: Some(DeviceCredentialMatch {
                device_id: DeviceId::generate(),
                matched_hash: token.token_hash(),
                revoked_at: None,
            }),
            fail_lookup: false,
            fail_touch: false,
            lookups: Mutex::new(0),
            touches: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn successful_auth_returns_only_device_and_touch_failure_is_nonfatal() {
        let token = BearerToken::parse("01".repeat(32)).unwrap();
        let mut store = store_for(&token);
        store.fail_touch = true;
        let expected = store.matched.unwrap().device_id;
        let authenticated = DeviceAuthenticator::new(&store)
            .authenticate(token, now())
            .await
            .unwrap();
        assert_eq!(authenticated.device_id(), expected);
        assert_eq!(store.touches.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unknown_revoked_mismatch_and_storage_collapse_to_one_safe_error() {
        let token_text = "01".repeat(32);
        let token = BearerToken::parse(token_text.clone()).unwrap();
        let matching_hash = token.token_hash();
        let other_hash = BearerToken::parse("02".repeat(32)).unwrap().token_hash();
        let device_id = DeviceId::generate();
        for store in [
            FakeStore {
                matched: None,
                fail_lookup: false,
                fail_touch: false,
                lookups: Mutex::new(0),
                touches: Mutex::new(Vec::new()),
            },
            FakeStore {
                matched: Some(DeviceCredentialMatch {
                    device_id,
                    matched_hash: matching_hash,
                    revoked_at: Some(now()),
                }),
                fail_lookup: false,
                fail_touch: false,
                lookups: Mutex::new(0),
                touches: Mutex::new(Vec::new()),
            },
            FakeStore {
                matched: Some(DeviceCredentialMatch {
                    device_id,
                    matched_hash: other_hash,
                    revoked_at: None,
                }),
                fail_lookup: false,
                fail_touch: false,
                lookups: Mutex::new(0),
                touches: Mutex::new(Vec::new()),
            },
            FakeStore {
                matched: None,
                fail_lookup: true,
                fail_touch: false,
                lookups: Mutex::new(0),
                touches: Mutex::new(Vec::new()),
            },
        ] {
            let error = DeviceAuthenticator::new(&store)
                .authenticate(BearerToken::parse(token_text.clone()).unwrap(), now())
                .await
                .unwrap_err();
            assert_eq!(error.code(), "authentication_failed");
            assert_eq!(error.to_string(), format!("{error:?}"));
            assert!(!error.to_string().contains(&token_text));
            assert!(store.touches.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn revoked_rows_still_execute_full_digest_comparison_before_uniform_failure() {
        let token = BearerToken::parse("01".repeat(32)).unwrap();
        let other_hash = BearerToken::parse("02".repeat(32)).unwrap().token_hash();
        let active_store = store_for(&token);
        let active_comparisons = Cell::new(0);
        let observe_active = || active_comparisons.set(active_comparisons.get() + 1);
        DeviceAuthenticator::with_digest_comparison_observer(&active_store, &observe_active)
            .authenticate(BearerToken::parse("01".repeat(32)).unwrap(), now())
            .await
            .unwrap();
        assert_eq!(active_comparisons.get(), 1);
        assert_eq!(active_store.touches.lock().unwrap().len(), 1);

        for matched_hash in [token.token_hash(), other_hash] {
            let store = FakeStore {
                matched: Some(DeviceCredentialMatch {
                    device_id: DeviceId::generate(),
                    matched_hash,
                    revoked_at: Some(now()),
                }),
                fail_lookup: false,
                fail_touch: false,
                lookups: Mutex::new(0),
                touches: Mutex::new(Vec::new()),
            };
            let comparisons = Cell::new(0);
            let observe = || comparisons.set(comparisons.get() + 1);
            let error = DeviceAuthenticator::with_digest_comparison_observer(&store, &observe)
                .authenticate(BearerToken::parse("01".repeat(32)).unwrap(), now())
                .await
                .unwrap_err();
            assert_eq!(comparisons.get(), 1);
            assert_eq!(error.kind(), AuthenticationErrorKind::AuthenticationFailed);
            assert_eq!(error.to_string(), "authentication_failed");
            assert!(store.touches.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn every_malformed_bearer_shape_collapses_before_persistence_lookup() {
        for rejected in [
            String::new(),
            "0".repeat(63),
            "0".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            format!(" {}", "0".repeat(64)),
            format!("{}\n", "0".repeat(64)),
            format!("Bearer {}", "0".repeat(64)),
        ] {
            let store = FakeStore {
                matched: None,
                fail_lookup: true,
                fail_touch: false,
                lookups: Mutex::new(0),
                touches: Mutex::new(Vec::new()),
            };
            let error = DeviceAuthenticator::new(&store)
                .authenticate_bearer(rejected, now())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), AuthenticationErrorKind::AuthenticationFailed);
            assert_eq!(format!("{error:?}"), "authentication_failed");
            assert_eq!(*store.lookups.lock().unwrap(), 0);
        }
    }
}
