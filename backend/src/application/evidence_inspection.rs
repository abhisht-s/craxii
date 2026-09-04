//! Deterministic rendering for redacted, noncanonical operator evidence.

use serde::Serialize;

use crate::domain::{RuntimeInstanceId, WorkId};
use crate::ports::artifact_store::ArtifactStore;
use crate::ports::evidence_query::{
    EvidenceExport, EvidencePreflight, EvidenceQueryError, EvidenceQueryStore, RuntimeEvidence,
    StateVerification, WorkEvidence,
};

pub const EVIDENCE_FORMAT_VERSION: &str = "craxii.operator-evidence/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceOutputFormat {
    Json,
    Markdown,
}

impl EvidenceOutputFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "json" => Some(Self::Json),
            "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct VersionedEvidence<'a, T: Serialize> {
    format_version: &'static str,
    artifact_kind: &'static str,
    evidence_role: &'static str,
    data: &'a T,
}

pub struct EvidenceInspectionService<'a> {
    store: &'a dyn EvidenceQueryStore,
    artifacts: &'a dyn ArtifactStore,
}

impl<'a> EvidenceInspectionService<'a> {
    #[must_use]
    pub const fn new(store: &'a dyn EvidenceQueryStore, artifacts: &'a dyn ArtifactStore) -> Self {
        Self { store, artifacts }
    }

    pub async fn preflight(
        &self,
        format: EvidenceOutputFormat,
    ) -> Result<String, EvidenceQueryError> {
        render("preflight", &self.store.preflight().await?, format)
    }

    pub async fn verify_state(
        &self,
        format: EvidenceOutputFormat,
    ) -> Result<(String, bool), EvidenceQueryError> {
        let report = self.store.verify_state(self.artifacts).await?;
        let consistent = report.consistent;
        Ok((render("verify_state", &report, format)?, consistent))
    }

    pub async fn inspect_work(
        &self,
        work_id: WorkId,
        format: EvidenceOutputFormat,
    ) -> Result<String, EvidenceQueryError> {
        render(
            "inspect_work",
            &self.store.inspect_work(work_id).await?,
            format,
        )
    }

    pub async fn inspect_runtime(
        &self,
        runtime_id: RuntimeInstanceId,
        format: EvidenceOutputFormat,
    ) -> Result<String, EvidenceQueryError> {
        render(
            "inspect_runtime",
            &self.store.inspect_runtime(runtime_id).await?,
            format,
        )
    }

    pub async fn export(&self, format: EvidenceOutputFormat) -> Result<String, EvidenceQueryError> {
        render(
            "evidence_export",
            &self.store.export(self.artifacts).await?,
            format,
        )
    }
}

fn render<T: Serialize>(
    kind: &'static str,
    value: &T,
    format: EvidenceOutputFormat,
) -> Result<String, EvidenceQueryError> {
    let document = VersionedEvidence {
        format_version: EVIDENCE_FORMAT_VERSION,
        artifact_kind: kind,
        evidence_role: "read_only_noncanonical",
        data: value,
    };
    let json = serde_json::to_string_pretty(&document).map_err(|_| {
        EvidenceQueryError::new(crate::ports::evidence_query::EvidenceQueryErrorKind::Integrity)
    })?;
    Ok(match format {
        EvidenceOutputFormat::Json => format!("{json}\n"),
        EvidenceOutputFormat::Markdown => format!(
            "# Craxii operator evidence\n\n- Format: `{}`\n- Kind: `{kind}`\n- Role: read-only, noncanonical\n\n```json\n{json}\n```\n",
            EVIDENCE_FORMAT_VERSION
        ),
    })
}

// Keep the concrete report types in this module's public API visible to rustdoc
// and make accidental replacement with arbitrary JSON less tempting.
const _: fn(EvidencePreflight, StateVerification, WorkEvidence, RuntimeEvidence, EvidenceExport) =
    |_, _, _, _, _| {};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::evidence_query::EvidencePreflight;

    #[test]
    fn stage23_rendering_is_versioned_deterministic_and_preserves_null() {
        let value = EvidencePreflight {
            schema_version: 4,
            database_disposition: "current",
            journal_head: None,
            work_count: 0,
            runtime_count: 0,
            model_attempt_count: 0,
            tool_execution_count: 0,
            artifact_count: 0,
        };
        let first = render("preflight", &value, EvidenceOutputFormat::Json).unwrap();
        let second = render("preflight", &value, EvidenceOutputFormat::Json).unwrap();
        assert_eq!(first, second);
        assert!(first.contains(EVIDENCE_FORMAT_VERSION));
        assert!(first.contains("\"journal_head\": null"));
        assert!(
            render("preflight", &value, EvidenceOutputFormat::Markdown)
                .unwrap()
                .starts_with("# Craxii operator evidence")
        );
    }
}
