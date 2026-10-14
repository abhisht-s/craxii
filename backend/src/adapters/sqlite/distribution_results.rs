//! Caller-transaction terminal export preparation and immutable recovery. No execution or I/O.
use super::{
    codec,
    error::{SqliteAdapterError, SqliteFailureKind},
    state_store::SqliteStateStore,
    transaction::WriteTransaction,
};
use crate::distribution_contracts::{
    canonical::{DigestDomain, canonical_bytes, digest_material, sha256},
    wire::decode_message,
};
use crate::domain::*;
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};
fn invalid() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}
fn canonical(v: &Value) -> Result<String, SqliteAdapterError> {
    String::from_utf8(canonical_bytes(v).map_err(|_| invalid())?).map_err(|_| invalid())
}
pub(super) fn reason(s: &WorkLifecycleSnapshot) -> Result<&'static str, SqliteAdapterError> {
    Ok(match s.terminal_reason().ok_or_else(invalid)? {
        WorkTerminalReason::Completion(r) => r.as_str(),
        WorkTerminalReason::Cancellation(r) => r.as_str(),
        WorkTerminalReason::Interruption(r) => r.as_str(),
        WorkTerminalReason::Failure(r) => match r {
            WorkFailureReason::ProviderExhausted => "model_attempts_exhausted",
            WorkFailureReason::InvalidModelOutput(_) => "invalid_model_output",
            WorkFailureReason::Limit(l) => match l {
                LifecycleLimit::Context => "context_limit",
                LifecycleLimit::ModelAttempts => "model_attempts_exhausted",
                LifecycleLimit::AgentLoopSteps => "agent_loop_limit",
                LifecycleLimit::ToolCalls => "tool_calls_limit",
                _ => "internal_failure",
            },
            WorkFailureReason::Definite(e) => {
                if e.category() == ErrorCategory::ProviderError {
                    "provider_failure"
                } else {
                    "internal_failure"
                }
            }
        },
    })
}
fn notice(state: WorkState, r: &str) -> Value {
    let text = match state {
        WorkState::Cancelled => "Work was cancelled.",
        WorkState::Failed => match r {
            "context_limit" => "Work failed because the context limit was reached.",
            "model_attempts_exhausted" => "Work failed because model attempts were exhausted.",
            "agent_loop_limit" => "Work failed because the agent loop limit was reached.",
            "tool_calls_limit" => "Work failed because the tool call limit was reached.",
            "invalid_model_output" => "Work failed because the model output was invalid.",
            "provider_failure" => "Work failed because the provider failed.",
            _ => "Work failed due to an internal failure.",
        },
        _ => match r {
            "runtime_ownership_lost" => {
                "Work was interrupted after runtime ownership was lost. Effects may have occurred."
            }
            "tool_interrupted_before_dispatch" => "Work was interrupted before tool dispatch.",
            _ => {
                "Work was interrupted with uncertain effects. Effects may have occurred; cleanup is not confirmed."
            }
        },
    };
    json!({"version":1,"blocks":[{"type":"text","text":text}]})
}
pub(super) async fn prepare(
    tx: &mut WriteTransaction,
    next: &WorkLifecycleSnapshot,
    event: JournalEventId,
    at: UtcTimestamp,
) -> Result<(), SqliteAdapterError> {
    if !next.state().is_terminal() {
        return Ok(());
    }
    let r = sqlx::query("SELECT * FROM distribution_receipts WHERE local_work_id=?")
        .bind(next.work_id().to_string())
        .fetch_optional(tx.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    let Some(r) = r else { return Ok(()) };
    if r.try_get::<i64, _>("quarantined")? != 0 {
        return Err(invalid());
    }
    let material: Value = serde_json::from_str(&r.try_get::<String, _>("material_canonical")?)
        .map_err(|_| invalid())?;
    let receipt: Value = serde_json::from_str(&r.try_get::<String, _>("receipt_canonical")?)
        .map_err(|_| invalid())?;
    let why = reason(next)?;
    let message = if next.state() == WorkState::Completed {
        Some(sqlx::query("SELECT message_id,content_json,content_sha256 FROM messages WHERE produced_by_work_id=? AND role='assistant'").bind(next.work_id().to_string()).fetch_one(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?)
    } else {
        None
    };
    let content = if let Some(m) = &message {
        let c = codec::decode_message_content(
            &m.try_get::<String, _>("content_json")?,
            &m.try_get::<String, _>("content_sha256")?,
        )?;
        if c.blocks().len() > 64 {
            return Err(invalid());
        }
        serde_json::from_str::<Value>(&codec::encode_message_content(&c)?.0)
            .map_err(|_| invalid())?
    } else {
        notice(next.state(), why)
    };
    let hash = sha256(&canonical_bytes(&content).map_err(|_| invalid())?);
    let export = uuid::Uuid::now_v7().to_string();
    let upload = crate::distribution::request_id().map_err(|_| invalid())?;
    let mut snapshot = json!({"snapshot_version":1,"environment":"development","customer_id":material["customer_id"],"logical_agent_id":material["logical_agent_id"],"work_id":material["work_id"],"conversation_id":material["conversation_id"],"origin_installation_id":receipt["origin_installation_id"],"receipt_id":receipt["receipt_id"],"local_work_id":receipt["local_work_id"],"export_id":export,"material_digest":receipt["material_digest"],"origin_generation":receipt["origin_generation"],"reply_binding":material["reply_binding"],"export_revision":"1","work_state_revision":next.projection_version().get().to_string(),"completed_at":at.to_string(),"content":content,"content_sha256":hash,"work_state":next.state().as_str(),"terminal_reason":why});
    if let Some(m) = &message {
        snapshot["local_message_id"] = json!(m.try_get::<String, _>("message_id")?)
    }
    let digest = digest_material(DigestDomain::Result, &snapshot).map_err(|_| invalid())?;
    let envelope = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"result_export","environment":"development","request_id":upload,"authority":{"customer_id":material["customer_id"],"logical_agent_id":material["logical_agent_id"],"installation_id":receipt["origin_installation_id"],"generation":receipt["origin_generation"],"session_id":upload},"body":{"snapshot":snapshot,"export_digest":digest,"upload_id":upload}});
    decode_message(&canonical_bytes(&envelope).map_err(|_| invalid())?).map_err(|_| invalid())?;
    snapshot
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("content");
    sqlx::query("INSERT INTO distribution_results(export_id,upload_id,environment,central_work_id,customer_id,logical_agent_id,central_conversation_id,receipt_id,local_work_id,work_state,state_version,terminal_reason,terminal_at,terminal_event_id,assistant_message_id,native_content_sha256,metadata_canonical,wire_content_sha256,export_digest,notice_content_canonical) VALUES(?,?,'development',?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
 .bind(export).bind(upload).bind(r.try_get::<String,_>("central_work_id")?).bind(r.try_get::<String,_>("customer_id")?).bind(r.try_get::<String,_>("logical_agent_id")?).bind(r.try_get::<String,_>("central_conversation_id")?).bind(r.try_get::<String,_>("receipt_id")?).bind(next.work_id().to_string()).bind(next.state().as_str()).bind(next.projection_version().get()).bind(why).bind(at.to_string()).bind(event.to_string()).bind(message.as_ref().map(|m|m.try_get::<String,_>("message_id")).transpose()?).bind(message.as_ref().map(|m|m.try_get::<String,_>("content_sha256")).transpose()?).bind(canonical(&snapshot)?).bind(hash).bind(digest).bind(if message.is_none(){Some(canonical(&content)?)}else{None}).execute(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?;
    Ok(())
}
async fn reconstruct(
    tx: &mut WriteTransaction,
    r: &SqliteRow,
) -> Result<Value, SqliteAdapterError> {
    let mut s: Value = serde_json::from_str(&r.try_get::<String, _>("metadata_canonical")?)
        .map_err(|_| invalid())?;
    let content = if let Some(id) = r.try_get::<Option<String>, _>("assistant_message_id")? {
        let m=sqlx::query("SELECT content_json,content_sha256,produced_by_work_id FROM messages WHERE message_id=?").bind(id).fetch_one(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?;
        if m.try_get::<String, _>("produced_by_work_id")?
            != r.try_get::<String, _>("local_work_id")?
            || m.try_get::<String, _>("content_sha256")?
                != r.try_get::<String, _>("native_content_sha256")?
        {
            return Err(invalid());
        }
        let c = codec::decode_message_content(
            &m.try_get::<String, _>("content_json")?,
            &m.try_get::<String, _>("content_sha256")?,
        )?;
        serde_json::from_str::<Value>(&codec::encode_message_content(&c)?.0)
            .map_err(|_| invalid())?
    } else {
        serde_json::from_str::<Value>(&r.try_get::<String, _>("notice_content_canonical")?)
            .map_err(|_| invalid())?
    };
    s["content"] = content;
    if sha256(&canonical_bytes(&s["content"]).map_err(|_| invalid())?)
        != r.try_get::<String, _>("wire_content_sha256")?
        || digest_material(DigestDomain::Result, &s).map_err(|_| invalid())?
            != r.try_get::<String, _>("export_digest")?
    {
        return Err(invalid());
    }
    let event =
        sqlx::query("SELECT work_id,event_type,payload_json FROM journal_events WHERE event_id=?")
            .bind(r.try_get::<String, _>("terminal_event_id")?)
            .fetch_one(tx.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    let p: Value = serde_json::from_str(&event.try_get::<String, _>("payload_json")?)
        .map_err(|_| invalid())?;
    if event.try_get::<String, _>("work_id")? != r.try_get::<String, _>("local_work_id")?
        || event.try_get::<String, _>("event_type")?
            != format!("work.{}", r.try_get::<String, _>("work_state")?)
        || p.get("transition").unwrap_or(&p)["state_version"].as_i64()
            != Some(r.try_get::<i64, _>("state_version")?)
    {
        return Err(invalid());
    }
    Ok(
        json!({"snapshot":s,"export_digest":r.try_get::<String,_>("export_digest")?,"upload_id":r.try_get::<String,_>("upload_id")?}),
    )
}
impl SqliteStateStore {
    pub async fn reconcile_distribution_results(&self) -> Result<(), SqliteAdapterError> {
        let mut tx =
            WriteTransaction::begin(&self.runtime, "reconcile_distribution_results").await?;
        let rows=sqlx::query("SELECT w.* FROM work_items w JOIN distribution_receipts r ON r.local_work_id=w.work_id LEFT JOIN distribution_results e ON e.local_work_id=w.work_id WHERE w.state IN ('completed','failed','cancelled','interrupted') AND e.export_id IS NULL").fetch_all(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?;
        for r in rows {
            let id_work = WorkId::parse_canonical(&r.try_get::<String, _>("work_id")?)
                .map_err(|_| invalid())?;
            let state = codec::decode_work_state(&r.try_get::<String, _>("state")?)?;
            let code = r.try_get::<String, _>("terminal_reason_code")?;
            let terminal = match state {
                WorkState::Completed => WorkTerminalReason::Completion(
                    serde_json::from_value(json!(code)).map_err(|_| invalid())?,
                ),
                WorkState::Cancelled => WorkTerminalReason::Cancellation(
                    serde_json::from_value(json!(code)).map_err(|_| invalid())?,
                ),
                WorkState::Interrupted => WorkTerminalReason::Interruption(
                    serde_json::from_value(json!(code)).map_err(|_| invalid())?,
                ),
                WorkState::Failed => {
                    let failure = match code.as_str() {
                        "provider_exhausted" => WorkFailureReason::ProviderExhausted,
                        "lifecycle_limit" => {
                            let detail: Value = serde_json::from_str(
                                &r.try_get::<String, _>("terminal_detail_json")?,
                            )
                            .map_err(|_| invalid())?;
                            WorkFailureReason::Limit(
                                serde_json::from_value(detail["limit"].clone())
                                    .map_err(|_| invalid())?,
                            )
                        }
                        "invalid_model_output" => WorkFailureReason::InvalidModelOutput(
                            ModelOutputFailure::ProviderFailed,
                        ),
                        _ => WorkFailureReason::Definite(NormalizedError::internal_invariant()),
                    };
                    WorkTerminalReason::Failure(failure)
                }
                _ => return Err(invalid()),
            };
            let lifecycle = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                work_id: id_work,
                state,
                projection_version: ProjectionVersion::try_new(r.try_get("state_version")?)
                    .map_err(|_| invalid())?,
                runtime_owner: None,
                current_attempt: CurrentWorkAttempt::None,
                cancellation_reason: None,
                terminal_reason: Some(terminal),
            })
            .map_err(|_| invalid())?;
            let at = codec::decode_timestamp(&r.try_get::<String, _>("terminal_at")?)?;
            let id:String=sqlx::query_scalar("SELECT event_id FROM journal_events WHERE work_id=? AND event_type=? ORDER BY journal_offset DESC LIMIT 1").bind(id_work.to_string()).bind(format!("work.{}",state)).fetch_one(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?;
            prepare(&mut tx, &lifecycle, id.parse().map_err(|_| invalid())?, at).await?;
        }
        tx.commit().await
    }
    pub async fn pending_distribution_results(&self) -> Result<Vec<Value>, SqliteAdapterError> {
        let mut tx = WriteTransaction::begin(&self.runtime, "pending_distribution_results").await?;
        let rows=sqlx::query("SELECT * FROM distribution_results WHERE transfer_state='pending' ORDER BY central_work_id LIMIT 4").fetch_all(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(reconstruct(&mut tx, &r).await?)
        }
        tx.commit().await?;
        Ok(out)
    }
    pub async fn acknowledge_distribution_result(
        &self,
        body: &Value,
    ) -> Result<(), SqliteAdapterError> {
        let mut tx =
            WriteTransaction::begin(&self.runtime, "acknowledge_distribution_result").await?;
        let r = sqlx::query("SELECT * FROM distribution_results WHERE export_id=?")
            .bind(body["export_id"].as_str().ok_or_else(invalid)?)
            .fetch_one(tx.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        let result = reconstruct(&mut tx, &r).await?;
        let s = &result["snapshot"];
        let envelope = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"result_acceptance","request_id":result["upload_id"],"environment":"development","authority":{"customer_id":s["customer_id"],"logical_agent_id":s["logical_agent_id"],"installation_id":s["origin_installation_id"],"generation":s["origin_generation"],"session_id":result["upload_id"]},"body":body});
        decode_message(&canonical_bytes(&envelope).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
        if body["work_id"] != s["work_id"]
            || body["export_revision"] != "1"
            || body["export_digest"] != result["export_digest"]
        {
            return Err(invalid());
        }
        for k in ["acceptance_id", "outbox_source_id"] {
            crate::distribution_contracts::primitives::uuid(
                body[k].as_str().ok_or_else(invalid)?,
                Some(4),
            )
            .map_err(|_| invalid())?;
        }
        crate::distribution_contracts::primitives::timestamp_micros(
            body["accepted_at"].as_str().ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?;
        let mut stable = body.clone();
        stable
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove("duplicate");
        let encoded = canonical(&stable)?;
        if r.try_get::<String, _>("transfer_state")? == "accepted" {
            if r.try_get::<String, _>("acceptance_body")? != encoded {
                return Err(invalid());
            }
            return tx.commit().await;
        }
        let affected=sqlx::query("UPDATE distribution_results SET transfer_state='accepted',last_transfer_outcome='accepted',acceptance_id=?,outbox_source_id=?,accepted_at=?,acceptance_body=? WHERE export_id=? AND transfer_state='pending'").bind(body["acceptance_id"].as_str()).bind(body["outbox_source_id"].as_str()).bind(body["accepted_at"].as_str()).bind(encoded).bind(body["export_id"].as_str()).execute(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?.rows_affected();
        if affected != 1 {
            return Err(invalid());
        }
        tx.commit().await
    }
    pub async fn record_distribution_result_transfer_failure(
        &self,
        export_id: &str,
        uncertain: bool,
        conflict: bool,
    ) -> Result<(), SqliteAdapterError> {
        let mut tx =
            WriteTransaction::begin(&self.runtime, "record_distribution_result_transfer_failure")
                .await?;
        sqlx::query("UPDATE distribution_results SET last_transfer_outcome=?,transfer_state=? WHERE export_id=? AND transfer_state='pending'").bind(if uncertain{"uncertain"}else{"held"}).bind(if conflict{"quarantined"}else{"pending"}).bind(export_id).execute(tx.connection()).await.map_err(SqliteAdapterError::from_sqlx)?;
        tx.commit().await
    }
    pub async fn verify_distribution_results(&self) -> Result<(), SqliteAdapterError> {
        let mut tx = WriteTransaction::begin(&self.runtime, "verify_distribution_results").await?;
        let rows = sqlx::query("SELECT * FROM distribution_results")
            .fetch_all(tx.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        for r in rows {
            reconstruct(&mut tx, &r).await?;
        }
        tx.commit().await
    }
}
