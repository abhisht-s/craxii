//! Typed, allowlisted values for operational telemetry.
//!
//! Constructors deliberately discard arbitrary text before a value can reach a
//! tracing field. Durable evidence may retain richer values; this module is only
//! the disposable operational-observation boundary.

use std::fmt::{self, Debug, Formatter};
use std::net::IpAddr;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::domain::{
    Certainty, ErrorCategory, ErrorCode, ModelTargetIdentity, NormalizedError, ProviderEvidenceId,
    Retryability, ToolName, ToolVersion,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeHostClass {
    Loopback,
    PrivateNetwork,
    PublicNetwork,
    Dns,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeUrlScheme {
    Http,
    Https,
    WebSocket,
    SecureWebSocket,
    Other,
    Unavailable,
}

/// URL facts that cannot retain userinfo, a host name, path parameters, query,
/// or fragment.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct SafeUrlSummary {
    scheme: SafeUrlScheme,
    host_class: SafeHostClass,
    port: Option<u16>,
    route_template: Option<&'static str>,
}

impl SafeUrlSummary {
    #[must_use]
    pub fn parse(value: &str, route_template: Option<&'static str>) -> Self {
        let Ok(url) = Url::parse(value) else {
            return Self {
                scheme: SafeUrlScheme::Unavailable,
                host_class: SafeHostClass::Unavailable,
                port: None,
                route_template,
            };
        };
        let scheme = match url.scheme() {
            "http" => SafeUrlScheme::Http,
            "https" => SafeUrlScheme::Https,
            "ws" => SafeUrlScheme::WebSocket,
            "wss" => SafeUrlScheme::SecureWebSocket,
            _ => SafeUrlScheme::Other,
        };
        let host_class = match url.host() {
            Some(Host::Domain(name)) if name.eq_ignore_ascii_case("localhost") => {
                SafeHostClass::Loopback
            }
            Some(Host::Domain(_)) => SafeHostClass::Dns,
            Some(Host::Ipv4(address)) => classify_ip(IpAddr::V4(address)),
            Some(Host::Ipv6(address)) => classify_ip(IpAddr::V6(address)),
            None => SafeHostClass::Unavailable,
        };
        Self {
            scheme,
            host_class,
            port: url.port(),
            route_template,
        }
    }

    #[must_use]
    pub const fn scheme(&self) -> SafeUrlScheme {
        self.scheme
    }

    #[must_use]
    pub const fn host_class(&self) -> SafeHostClass {
        self.host_class
    }

    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        self.port
    }

    #[must_use]
    pub const fn route_template(&self) -> Option<&'static str> {
        self.route_template
    }
}

impl Debug for SafeUrlSummary {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SafeUrlSummary")
            .field("scheme", &self.scheme)
            .field("host_class", &self.host_class)
            .field("port", &self.port)
            .field("route_template", &self.route_template)
            .finish()
    }
}

fn classify_ip(address: IpAddr) -> SafeHostClass {
    if address.is_loopback() {
        SafeHostClass::Loopback
    } else if match address {
        IpAddr::V4(value) => value.is_private() || value.is_link_local(),
        IpAddr::V6(value) => value.is_unique_local() || value.is_unicast_link_local(),
    } {
        SafeHostClass::PrivateNetwork
    } else {
        SafeHostClass::PublicNetwork
    }
}

/// One-way digest for a provider-controlled request/response identifier.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SafeProviderCorrelation(String);

impl SafeProviderCorrelation {
    #[must_use]
    pub fn from_provider_id(value: &ProviderEvidenceId) -> Self {
        Self::from_untrusted(value.as_str())
    }

    #[must_use]
    pub fn from_untrusted(value: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"craxii-provider-correlation-v1\0");
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        let mut encoded = String::with_capacity(7 + digest.len() * 2);
        encoded.push_str("sha256:");
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(encoded)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for SafeProviderCorrelation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SafeProviderCorrelation")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafePathKind {
    State,
    Artifact,
    Workspace,
    Temporary,
    Configuration,
    Other,
}

/// Logical path classification only. The source path is never retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SafePathClassification {
    kind: SafePathKind,
    within_workspace: Option<bool>,
}

impl SafePathClassification {
    #[must_use]
    pub fn new(kind: SafePathKind, path: &Path, workspace: Option<&Path>) -> Self {
        Self {
            kind,
            within_workspace: workspace.map(|root| path.starts_with(root)),
        }
    }

    #[must_use]
    pub const fn kind(self) -> SafePathKind {
        self.kind
    }

    #[must_use]
    pub const fn within_workspace(self) -> Option<bool> {
        self.within_workspace
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SafeErrorSummary {
    pub category: ErrorCategory,
    pub code: ErrorCode,
    pub retryability: Retryability,
    pub certainty: Certainty,
    pub source_status: Option<i32>,
}

impl From<&NormalizedError> for SafeErrorSummary {
    fn from(error: &NormalizedError) -> Self {
        Self {
            category: error.category(),
            code: error.code(),
            retryability: error.retryability(),
            certainty: error.certainty(),
            source_status: error.source_status().map(|status| status.code()),
        }
    }
}

/// Safe model observation: validated configuration identifiers plus numeric facts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeModelObservation {
    provider: String,
    model: String,
    target: String,
    request_bytes: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl SafeModelObservation {
    #[must_use]
    pub fn new(
        identity: &ModelTargetIdentity,
        request_bytes: Option<u64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    ) -> Self {
        Self {
            provider: identity.provider_id().as_str().to_owned(),
            model: identity.provider_model_id().as_str().to_owned(),
            target: identity.model_target_id().as_str().to_owned(),
            request_bytes,
            input_tokens,
            output_tokens,
        }
    }
}

/// Safe tool observation. Arguments and result content have no representable field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeToolObservation {
    name: String,
    version: String,
    output_bytes: Option<u64>,
    truncated: Option<bool>,
}

impl SafeToolObservation {
    #[must_use]
    pub fn new(
        name: &ToolName,
        version: &ToolVersion,
        output_bytes: Option<u64>,
        truncated: Option<bool>,
    ) -> Self {
        Self {
            name: name.as_str().to_owned(),
            version: version.as_str().to_owned(),
            output_bytes,
            truncated,
        }
    }
}

/// Safe child-process/workstation facts. Commands, output, environment and paths
/// have no representable field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeWorkstationObservation {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout_bytes: Option<u64>,
    pub stderr_bytes: Option<u64>,
    pub timed_out: bool,
    pub cancelled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINELS: [&str; 8] = [
        "username-sentinel",
        "password-sentinel",
        "host-sentinel.example",
        "path-sentinel",
        "query-sentinel",
        "fragment-sentinel",
        "provider-request-sentinel",
        "/Users/private/absolute-sentinel",
    ];

    #[test]
    fn stage23_url_summary_is_an_allowlist_not_a_string_redactor() {
        let cases = [
            "https://username-sentinel:password-sentinel@host-sentinel.example:8443/path-sentinel?secret=query-sentinel#fragment-sentinel",
            "http://127.0.0.1/path-sentinel?query-sentinel",
            "wss://[::1]/fragment-sentinel#secret",
            "not a URL with password-sentinel",
        ];
        for value in cases {
            let rendered = format!("{:?}", SafeUrlSummary::parse(value, Some("/v1/items/:id")));
            for sentinel in SENTINELS {
                assert!(
                    !rendered.contains(sentinel),
                    "leaked {sentinel}: {rendered}"
                );
            }
        }
    }

    #[test]
    fn stage23_provider_correlation_is_one_way_and_stable() {
        let raw = ProviderEvidenceId::try_new("provider-request-sentinel").unwrap();
        let left = SafeProviderCorrelation::from_provider_id(&raw);
        let right = SafeProviderCorrelation::from_provider_id(&raw);
        assert_eq!(left, right);
        assert!(left.as_str().starts_with("sha256:"));
        assert!(!format!("{left:?}").contains(raw.as_str()));
    }

    #[test]
    fn stage23_path_classification_does_not_retain_the_path() {
        let value = SafePathClassification::new(
            SafePathKind::Workspace,
            Path::new("/Users/private/absolute-sentinel/file"),
            Some(Path::new("/Users/private/absolute-sentinel")),
        );
        let rendered = format!("{value:?}");
        assert_eq!(value.within_workspace(), Some(true));
        assert!(!rendered.contains("absolute-sentinel"));
    }
}
