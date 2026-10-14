//! Distribution evidence in the existing BEGIN IMMEDIATE admission/cancellation transaction.
use super::cancellation::{
    CancellationJournalOrigin, apply_cancellation_decision, decode_loaded_cancellation_work,
};
use super::message_admission::{
    CanonicalMessageAdmission, CanonicalMessageCandidates, CanonicalMessageOrigin,
    CanonicalMessageTopology, admit_canonical_message,
};
use super::{state_store::SqliteStateStore, transaction::WriteTransaction};
use crate::distribution::{
    DistributionError, DistributionResult, ValidatedDelivery, text, timestamp,
};
use crate::distribution_contracts::canonical::{DigestDomain, canonical_bytes, digest_material};
use crate::domain::{
    ContentBlock, ConversationId, ConversationWorkOrdinal, CraxiiId, JournalEventId,
    MessageContent, MessageId, ProjectionVersion, UserId, WorkId, WorkspaceId,
};
use serde_json::{Value, json};
use sqlx::Row;
fn storage(_: impl std::fmt::Debug) -> DistributionError {
    DistributionError::Storage
}
fn invalid() -> DistributionError {
    DistributionError::Invalid
}
fn string(v: &Value) -> DistributionResult<String> {
    Ok(String::from_utf8(canonical_bytes(v).map_err(|_| invalid())?).map_err(|_| invalid())?)
}
fn parse(s: &str) -> DistributionResult<Value> {
    crate::distribution_contracts::raw_json::parse_bounded(
        s.as_bytes(),
        crate::distribution_contracts::raw_json::LARGE_LIMITS,
    )
    .map_err(|_| invalid())
}
fn same(a: &Value, b: &Value) -> DistributionResult<bool> {
    Ok(canonical_bytes(a).map_err(|_| invalid())? == canonical_bytes(b).map_err(|_| invalid())?)
}
#[derive(Clone, Debug)]
pub enum DistributionAdmission {
    Receipt {
        receipt: Value,
        duplicate: bool,
        state: String,
        revision: String,
    },
    Cancelled {
        control_receipt: Value,
    },
}
impl SqliteStateStore {
    async fn distribution_owner(
        &self,
        tx: &mut WriteTransaction,
        m: &Value,
        a: &Value,
    ) -> DistributionResult<(String, String, String)> {
        let generation = text(a, "generation")?
            .parse::<i64>()
            .map_err(|_| invalid())?;
        let installation = text(a, "installation_id")?;
        let environment = text(m, "environment")?;
        let customer = text(m, "customer_id")?;
        let agent = text(m, "logical_agent_id")?;
        if environment != "development" {
            return Err(invalid());
        }
        let root=sqlx::query("SELECT p.craxii_id,p.primary_conversation_id,c.owner_user_id FROM craxii_principals p JOIN conversations c ON c.conversation_id=p.primary_conversation_id").fetch_one(tx.connection()).await.map_err(storage)?;
        let user: String = root.try_get("owner_user_id").map_err(storage)?;
        let craxii: String = root.try_get("craxii_id").map_err(storage)?;
        let conversation: String = root.try_get("primary_conversation_id").map_err(storage)?;
        let owners = sqlx::query("SELECT * FROM distribution_owners")
            .fetch_all(tx.connection())
            .await
            .map_err(storage)?;
        if let Some(owner) = owners.first() {
            if owners.len() != 1
                || owner.try_get::<String, _>("environment").map_err(storage)? != environment
                || owner.try_get::<String, _>("customer_id").map_err(storage)? != customer
                || owner
                    .try_get::<String, _>("logical_agent_id")
                    .map_err(storage)?
                    != agent
                || owner.try_get::<String, _>("user_id").map_err(storage)? != user
                || owner.try_get::<String, _>("craxii_id").map_err(storage)? != craxii
            {
                return Err(invalid());
            }
            let highest = owner
                .try_get::<i64, _>("highest_generation")
                .map_err(storage)?;
            if generation < highest
                || (generation == highest
                    && owner
                        .try_get::<String, _>("current_installation_id")
                        .map_err(storage)?
                        != installation)
            {
                return Err(invalid());
            }
            if generation > highest {
                sqlx::query("UPDATE distribution_owners SET highest_generation=?,current_installation_id=? WHERE environment=? AND customer_id=? AND logical_agent_id=?").bind(generation).bind(installation).bind(environment).bind(customer).bind(agent).execute(tx.connection()).await.map_err(storage)?;
            }
        } else {
            sqlx::query("INSERT INTO distribution_owners(environment,customer_id,logical_agent_id,user_id,craxii_id,highest_generation,current_installation_id) VALUES(?,?,?,?,?,?,?)").bind(environment).bind(customer).bind(agent).bind(&user).bind(&craxii).bind(generation).bind(installation).execute(tx.connection()).await.map_err(storage)?;
        }
        Ok((user, craxii, conversation))
    }
    async fn distribution_conversation(
        &self,
        tx: &mut WriteTransaction,
        m: &Value,
        user: &str,
        craxii: &str,
        conversation: &str,
    ) -> DistributionResult<i64> {
        let central = text(m, "conversation_id")?;
        sqlx::query("INSERT INTO distribution_conversations(environment,customer_id,logical_agent_id,central_conversation_id,user_id,craxii_id,local_conversation_id) VALUES('development',?,?,?,?,?,?) ON CONFLICT(environment,central_conversation_id) DO NOTHING").bind(text(m,"customer_id")?).bind(text(m,"logical_agent_id")?).bind(central).bind(user).bind(craxii).bind(conversation).execute(tx.connection()).await.map_err(storage)?;
        let row=sqlx::query("SELECT * FROM distribution_conversations WHERE environment='development' AND central_conversation_id=?").bind(central).fetch_one(tx.connection()).await.map_err(storage)?;
        if row.try_get::<String, _>("customer_id").map_err(storage)? != text(m, "customer_id")?
            || row
                .try_get::<String, _>("logical_agent_id")
                .map_err(storage)?
                != text(m, "logical_agent_id")?
            || row
                .try_get::<String, _>("local_conversation_id")
                .map_err(storage)?
                != conversation
        {
            return Err(invalid());
        }
        row.try_get("next_sequence").map_err(storage)
    }
    pub async fn admit_distribution(
        &self,
        delivery: &ValidatedDelivery,
    ) -> DistributionResult<DistributionAdmission> {
        if delivery.message["type"] != "work_delivery" {
            return Err(invalid());
        }
        delivery.check_fresh()?;
        crate::distribution_contracts::wire::decode_message(
            &canonical_bytes(&delivery.message).map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?;
        let b = &delivery.message["body"];
        let m = &b["material"];
        let id = text(m, "work_id")?;
        let digest = text(b, "material_digest")?;
        let mut tx = WriteTransaction::begin(&self.runtime, "distribution_admission")
            .await
            .map_err(storage)?;
        delivery.check_fresh()?;
        if sqlx::query_scalar::<_,i64>("SELECT (SELECT count(*) FROM distribution_receipts WHERE quarantined=1)+(SELECT count(*) FROM distribution_controls WHERE quarantined=1)").fetch_one(tx.connection()).await.map_err(storage)?!=0{return Err(DistributionError::Conflict);}

        let (user, craxii, conversation) = self
            .distribution_owner(&mut tx, m, &delivery.message["authority"])
            .await?;
        if let Some(row)=sqlx::query("SELECT r.*,w.state,w.state_version FROM distribution_receipts r JOIN work_items w ON w.work_id=r.local_work_id WHERE r.environment='development' AND r.central_work_id=?").bind(id).fetch_optional(tx.connection()).await.map_err(storage)? {
   if row.try_get::<String,_>("customer_id").map_err(storage)?!=text(m,"customer_id")?||row.try_get::<String,_>("logical_agent_id").map_err(storage)?!=text(m,"logical_agent_id")?||row.try_get::<String,_>("material_digest").map_err(storage)?!=digest||row.try_get::<String,_>("material_canonical").map_err(storage)?!=string(m)?{sqlx::query("UPDATE distribution_receipts SET quarantined=1 WHERE environment='development' AND central_work_id=?").bind(id).execute(tx.connection()).await.map_err(storage)?;tx.commit().await.map_err(storage)?;return Err(DistributionError::Conflict);}
   if row.try_get::<i64,_>("quarantined").map_err(storage)?!=0{return Err(DistributionError::Conflict);}
   let result=DistributionAdmission::Receipt{receipt:parse(&row.try_get::<String,_>("receipt_canonical").map_err(storage)?)?,duplicate:true,state:row.try_get("state").map_err(storage)?,revision:row.try_get::<i64,_>("state_version").map_err(storage)?.to_string()};tx.commit().await.map_err(storage)?;return Ok(result);
  }
        let next = self
            .distribution_conversation(&mut tx, m, &user, &craxii, &conversation)
            .await?;
        let sequence = text(m, "conversation_sequence")?
            .parse::<i64>()
            .map_err(|_| invalid())?;
        let controls=sqlx::query("SELECT * FROM distribution_controls WHERE environment='development' AND central_work_id=? ORDER BY control_id").bind(id).fetch_all(tx.connection()).await.map_err(storage)?;
        if let Some(control) = controls.first() {
            if control
                .try_get::<String, _>("customer_id")
                .map_err(storage)?
                != text(m, "customer_id")?
                || control
                    .try_get::<String, _>("logical_agent_id")
                    .map_err(storage)?
                    != text(m, "logical_agent_id")?
                || control
                    .try_get::<String, _>("material_digest")
                    .map_err(storage)?
                    != digest
            {
                return Err(DistributionError::Conflict);
            }
            if sequence > next {
                return Err(DistributionError::Reconciliation);
            }
            if sequence == next {
                sqlx::query("UPDATE distribution_conversations SET next_sequence=next_sequence+1 WHERE environment='development' AND central_conversation_id=? AND next_sequence=?").bind(text(m,"conversation_id")?).bind(sequence).execute(tx.connection()).await.map_err(storage)?;
            }
            sqlx::query("UPDATE distribution_controls SET sequence_reconciled=1,central_conversation_id=?,central_sequence=?,target_material_canonical=? WHERE environment='development' AND central_work_id=?").bind(text(m,"conversation_id")?).bind(sequence).bind(string(m)?).bind(id).execute(tx.connection()).await.map_err(storage)?;
            let receipt = parse(
                &control
                    .try_get::<String, _>("receipt_canonical")
                    .map_err(storage)?,
            )?;
            delivery.check_fresh()?;
            tx.commit().await.map_err(storage)?;
            return Ok(DistributionAdmission::Cancelled {
                control_receipt: receipt,
            });
        }
        if sequence != next {
            return Err(DistributionError::Reconciliation);
        }
        delivery.verify_profile()?;
        delivery.check_fresh()?;
        let row=sqlx::query("SELECT c.next_work_ordinal,c.state_version,p.default_workspace_id,je.event_id FROM conversations c JOIN craxii_principals p ON p.craxii_id=c.craxii_id JOIN journal_events je ON je.conversation_id=c.conversation_id AND je.event_type='conversation.created' WHERE c.conversation_id=?").bind(&conversation).fetch_one(tx.connection()).await.map_err(storage)?;
        let receipt_id = JournalEventId::generate();
        let accepted_event = JournalEventId::generate();
        let at = timestamp()?;
        let blocks = m["content"]["blocks"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|block| Ok(ContentBlock::Text(text(block, "text")?.to_owned())))
            .collect::<DistributionResult<Vec<_>>>()?;
        let material = string(m)?;
        let admitted = admit_canonical_message(
            &mut tx,
            CanonicalMessageAdmission {
                topology: CanonicalMessageTopology {
                    craxii_id: CraxiiId::parse_canonical(&craxii).map_err(|_| invalid())?,
                    user_id: UserId::parse_canonical(&user).map_err(|_| invalid())?,
                    conversation_id: ConversationId::parse_canonical(&conversation)
                        .map_err(|_| invalid())?,
                    workspace_id: WorkspaceId::parse_canonical(
                        &row.try_get::<String, _>("default_workspace_id")
                            .map_err(storage)?,
                    )
                    .map_err(|_| invalid())?,
                    next_ordinal: ConversationWorkOrdinal::try_new(
                        row.try_get("next_work_ordinal").map_err(storage)?,
                    )
                    .map_err(|_| invalid())?,
                    conversation_version: ProjectionVersion::try_new(
                        row.try_get("state_version").map_err(storage)?,
                    )
                    .map_err(|_| invalid())?,
                    conversation_created_event_id: JournalEventId::parse_canonical(
                        &row.try_get::<String, _>("event_id").map_err(storage)?,
                    )
                    .map_err(|_| invalid())?,
                },
                origin: CanonicalMessageOrigin::Distribution { receipt_id },
                content: MessageContent::try_new(blocks).map_err(|_| invalid())?,
                reply_binding_id: None,
                admitted_at: at,
                candidates: CanonicalMessageCandidates {
                    message_id: MessageId::generate(),
                    work_id: WorkId::generate(),
                    acceptance_event_id: accepted_event,
                    queued_event_id: JournalEventId::generate(),
                },
            },
            |_| Ok(()),
        )
        .await
        .map_err(storage)?;
        let a = &delivery.message["authority"];
        let receipt = json!({"work_id":id,"origin_installation_id":a["installation_id"],"receipt_id":receipt_id.to_string(),"local_work_id":admitted.work_id.to_string(),"local_message_id":admitted.message_id.to_string(),"local_conversation_id":conversation,"conversation_sequence":m["conversation_sequence"],"local_work_ordinal":admitted.work_ordinal.get().to_string(),"journal_offset":admitted.committed_cursor.get().to_string(),"origin_generation":a["generation"],"material_digest":digest,"admitted_at":at.to_string()});
        sqlx::query("INSERT INTO distribution_receipts(environment,central_work_id,customer_id,logical_agent_id,central_conversation_id,central_sequence,material_digest,material_canonical,receipt_id,receipt_canonical,user_id,local_conversation_id,local_message_id,local_work_id,acceptance_event_id,journal_offset,local_work_ordinal,origin_installation_id,origin_generation,admitted_at) VALUES('development',?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
   .bind(id).bind(text(m,"customer_id")?).bind(text(m,"logical_agent_id")?).bind(text(m,"conversation_id")?).bind(sequence).bind(digest).bind(material).bind(receipt_id.to_string()).bind(string(&receipt)?).bind(user).bind(conversation).bind(admitted.message_id.to_string()).bind(admitted.work_id.to_string()).bind(accepted_event.to_string()).bind(admitted.committed_cursor.get()).bind(admitted.work_ordinal.get()).bind(text(a,"installation_id")?).bind(text(a,"generation")?.parse::<i64>().map_err(|_|invalid())?).bind(at.to_string()).execute(tx.connection()).await.map_err(storage)?;
        sqlx::query("UPDATE distribution_conversations SET next_sequence=next_sequence+1 WHERE environment='development' AND central_conversation_id=? AND next_sequence=?").bind(text(m,"conversation_id")?).bind(sequence).execute(tx.connection()).await.map_err(storage)?;
        delivery.check_fresh()?;
        delivery.verify_profile()?;
        tx.commit().await.map_err(storage)?;
        Ok(DistributionAdmission::Receipt {
            receipt,
            duplicate: false,
            state: "queued".into(),
            revision: "1".into(),
        })
    }
    pub async fn cancel_distribution(
        &self,
        delivery: &ValidatedDelivery,
    ) -> DistributionResult<Value> {
        if delivery.message["type"] != "cancel_delivery" {
            return Err(invalid());
        }
        delivery.check_fresh()?;
        let b = &delivery.message["body"];
        let c = &b["control"];
        let id = text(c, "control_id")?;
        let work = text(c, "work_id")?;
        let digest = text(b, "control_digest")?;
        let mut tx = WriteTransaction::begin(&self.runtime, "distribution_cancel")
            .await
            .map_err(storage)?;
        delivery.check_fresh()?;
        if sqlx::query_scalar::<_,i64>("SELECT (SELECT count(*) FROM distribution_receipts WHERE quarantined=1)+(SELECT count(*) FROM distribution_controls WHERE quarantined=1)").fetch_one(tx.connection()).await.map_err(storage)?!=0{return Err(DistributionError::Conflict);}

        let mut owner = c.clone();
        owner["environment"] = json!("development");
        let (user, _, _) = self
            .distribution_owner(&mut tx, &owner, &delivery.message["authority"])
            .await?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM distribution_controls WHERE environment='development' AND control_id=?",
        )
        .bind(id)
        .fetch_optional(tx.connection())
        .await
        .map_err(storage)?
        {
            if row
                .try_get::<String, _>("control_digest")
                .map_err(storage)?
                != digest
                || row
                    .try_get::<String, _>("control_canonical")
                    .map_err(storage)?
                    != string(c)?
            {
                sqlx::query("UPDATE distribution_controls SET quarantined=1 WHERE environment='development' AND control_id=?").bind(id).execute(tx.connection()).await.map_err(storage)?;
                tx.commit().await.map_err(storage)?;
                return Err(DistributionError::Conflict);
            }
            if row.try_get::<i64, _>("quarantined").map_err(storage)? != 0 {
                return Err(DistributionError::Conflict);
            }
            let receipt = parse(
                &row.try_get::<String, _>("receipt_canonical")
                    .map_err(storage)?,
            )?;
            tx.commit().await.map_err(storage)?;
            return Ok(receipt);
        }
        let local=sqlx::query("SELECT * FROM distribution_receipts WHERE environment='development' AND central_work_id=?").bind(work).fetch_optional(tx.connection()).await.map_err(storage)?;
        let at = timestamp()?;
        let mut receipt = json!({"control_id":id,"work_id":work,"control_digest":digest,"committed_at":at.to_string(),"cleanup_pending":false,"outcome":"cancelled_before_admission"});
        let mut local_id = None;
        let mut event_id = None;
        if let Some(local) = local {
            if local.try_get::<String, _>("customer_id").map_err(storage)?
                != text(c, "customer_id")?
                || local
                    .try_get::<String, _>("logical_agent_id")
                    .map_err(storage)?
                    != text(c, "logical_agent_id")?
                || local
                    .try_get::<String, _>("material_digest")
                    .map_err(storage)?
                    != text(c, "material_digest")?
            {
                return Err(DistributionError::Conflict);
            }
            let w = local
                .try_get::<String, _>("local_work_id")
                .map_err(storage)?;
            let work_id = WorkId::parse_canonical(&w).map_err(|_| invalid())?;
            let row = sqlx::query("SELECT * FROM work_items WHERE work_id=?")
                .bind(&w)
                .fetch_one(tx.connection())
                .await
                .map_err(storage)?;
            let loaded = decode_loaded_cancellation_work(&row, work_id).map_err(storage)?;
            let before = loaded.snapshot.state();
            let candidate = JournalEventId::generate();
            let mutation = apply_cancellation_decision(
                &mut tx,
                &loaded,
                at,
                candidate,
                CancellationJournalOrigin::Distribution {
                    user_id: UserId::parse_canonical(&user).map_err(|_| invalid())?,
                    control_id: uuid::Uuid::parse_str(id).map_err(|_| invalid())?,
                },
            )
            .await
            .map_err(storage)?;
            let row = sqlx::query("SELECT state,state_version FROM work_items WHERE work_id=?")
                .bind(&w)
                .fetch_one(tx.connection())
                .await
                .map_err(storage)?;
            let state: String = row.try_get("state").map_err(storage)?;
            let outcome = if before.is_terminal() {
                "already_terminal"
            } else if before == crate::domain::WorkState::CancelRequested {
                "already_requested"
            } else if mutation.resulting_state == crate::domain::WorkState::Cancelled {
                "cancelled"
            } else {
                "requested"
            };
            receipt["outcome"] = json!(outcome);
            receipt["cleanup_pending"] = json!(state == "cancel_requested");
            receipt["local_work_id"] = json!(w);
            receipt["work_state"] = json!(state);
            receipt["work_state_revision"] = json!(
                row.try_get::<i64, _>("state_version")
                    .map_err(storage)?
                    .to_string()
            );
            local_id = Some(w);
            event_id = mutation.committed_cursor.map(|_| candidate.to_string());
        }
        sqlx::query("INSERT INTO distribution_controls(environment,control_id,customer_id,logical_agent_id,central_work_id,material_digest,control_digest,control_canonical,receipt_canonical,local_work_id,cancellation_event_id,sequence_reconciled) VALUES('development',?,?,?,?,?,?,?,?,?,?,?)").bind(id).bind(text(c,"customer_id")?).bind(text(c,"logical_agent_id")?).bind(work).bind(text(c,"material_digest")?).bind(digest).bind(string(c)?).bind(string(&receipt)?).bind(&local_id).bind(event_id).bind(i64::from(local_id.is_some())).execute(tx.connection()).await.map_err(storage)?;
        delivery.check_fresh()?;
        tx.commit().await.map_err(storage)?;
        Ok(receipt)
    }
    pub async fn distribution_observation(&self) -> DistributionResult<Value> {
        let mut tx = WriteTransaction::begin(&self.runtime, "distribution_observation")
            .await
            .map_err(storage)?;
        let r=sqlx::query("SELECT (SELECT count(*) FROM work_items WHERE state='queued') queued,(SELECT count(*) FROM work_items WHERE state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested')) active,(SELECT count(*) FROM work_items WHERE state='cancel_requested') cancel_requested,(SELECT count(*) FROM distribution_receipts WHERE central_acknowledged=0) pending_receipts,(SELECT count(*) FROM distribution_controls WHERE central_acknowledged=0) pending_controls,(SELECT count(*) FROM distribution_results WHERE transfer_state='pending') pending_exports,(SELECT count(*) FROM distribution_results WHERE transfer_state='accepted') accepted_exports,(SELECT count(*) FROM distribution_results WHERE transfer_state='quarantined') unresolved_exports,(SELECT count(*) FROM distribution_results WHERE transfer_state='pending' AND last_transfer_outcome='uncertain') uncertain_exports").fetch_one(tx.connection()).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(
            json!({"queued_work_count":r.try_get::<i64,_>("queued").map_err(storage)?.to_string(),"active_work_count":r.try_get::<i64,_>("active").map_err(storage)?.to_string(),"cancel_requested_count":r.try_get::<i64,_>("cancel_requested").map_err(storage)?.to_string(),"pending_receipt_ack_count":r.try_get::<i64,_>("pending_receipts").map_err(storage)?.to_string(),"pending_control_ack_count":r.try_get::<i64,_>("pending_controls").map_err(storage)?.to_string(),"pending_export_count":r.try_get::<i64,_>("pending_exports").map_err(storage)?.to_string(),"accepted_export_count":r.try_get::<i64,_>("accepted_exports").map_err(storage)?.to_string(),"unresolved_export_count":r.try_get::<i64,_>("unresolved_exports").map_err(storage)?.to_string(),"uncertain_export_count":r.try_get::<i64,_>("uncertain_exports").map_err(storage)?.to_string(),"claims_closed":true}),
        )
    }
    pub async fn assert_distribution_identity(
        &self,
        customer: &str,
        agent: &str,
    ) -> DistributionResult<()> {
        let rows = sqlx::query("SELECT customer_id,logical_agent_id FROM distribution_owners")
            .fetch_all(&self.runtime.inner.pool)
            .await
            .map_err(storage)?;
        if rows.len() > 1
            || rows.first().is_some_and(|r| {
                r.try_get::<String, _>("customer_id").ok().as_deref() != Some(customer)
                    || r.try_get::<String, _>("logical_agent_id").ok().as_deref() != Some(agent)
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub async fn pending_distribution_receipts(
        &self,
    ) -> DistributionResult<Vec<DistributionAdmission>> {
        let rows=sqlx::query("SELECT r.receipt_canonical,w.state,w.state_version FROM distribution_receipts r JOIN work_items w ON w.work_id=r.local_work_id WHERE central_acknowledged=0 ORDER BY central_sequence LIMIT 4").fetch_all(&self.runtime.inner.pool).await.map_err(storage)?;
        rows.iter()
            .map(|r| {
                Ok(DistributionAdmission::Receipt {
                    receipt: parse(
                        &r.try_get::<String, _>("receipt_canonical")
                            .map_err(storage)?,
                    )?,
                    duplicate: true,
                    state: r.try_get("state").map_err(storage)?,
                    revision: r
                        .try_get::<i64, _>("state_version")
                        .map_err(storage)?
                        .to_string(),
                })
            })
            .collect()
    }
    pub async fn pending_distribution_controls(&self) -> DistributionResult<Vec<Value>> {
        let rows=sqlx::query("SELECT receipt_canonical FROM distribution_controls WHERE central_acknowledged=0 AND (local_work_id IS NOT NULL OR sequence_reconciled=1) ORDER BY control_id LIMIT 4").fetch_all(&self.runtime.inner.pool).await.map_err(storage)?;
        rows.iter()
            .map(|r| {
                parse(
                    &r.try_get::<String, _>("receipt_canonical")
                        .map_err(storage)?,
                )
            })
            .collect()
    }
    pub async fn acknowledge_distribution(
        &self,
        receipt: &Value,
        control: bool,
    ) -> DistributionResult<()> {
        let mut tx = WriteTransaction::begin(&self.runtime, "distribution_transfer_ack")
            .await
            .map_err(storage)?;
        let (read, update, key) = if control {
            (
                "SELECT receipt_canonical FROM distribution_controls WHERE environment='development' AND control_id=?",
                "UPDATE distribution_controls SET central_acknowledged=1 WHERE environment='development' AND control_id=?",
                text(receipt, "control_id")?,
            )
        } else {
            (
                "SELECT receipt_canonical FROM distribution_receipts WHERE environment='development' AND central_work_id=?",
                "UPDATE distribution_receipts SET central_acknowledged=1 WHERE environment='development' AND central_work_id=?",
                text(receipt, "work_id")?,
            )
        };
        let row = sqlx::query(read)
            .bind(key)
            .fetch_one(tx.connection())
            .await
            .map_err(storage)?;
        if !same(
            receipt,
            &parse(
                &row.try_get::<String, _>("receipt_canonical")
                    .map_err(storage)?,
            )?,
        )? {
            return Err(DistributionError::Conflict);
        }
        sqlx::query(update)
            .bind(key)
            .execute(tx.connection())
            .await
            .map_err(storage)?;
        tx.commit().await.map_err(storage)
    }
    pub async fn verify_distribution_consistency(&self) -> DistributionResult<()> {
        if sqlx::query_scalar::<_,i64>("SELECT (SELECT count(*) FROM distribution_receipts WHERE quarantined=1)+(SELECT count(*) FROM distribution_controls WHERE quarantined=1)").fetch_one(&self.runtime.inner.pool).await.map_err(storage)?!=0{return Err(DistributionError::Conflict);}

        let rows=sqlx::query("SELECT r.*,m.*,w.conversation_work_ordinal,w.reply_binding_id,je.event_type,je.event_version,je.payload_json,qi.input_event_id FROM distribution_receipts r JOIN messages m ON m.message_id=r.local_message_id JOIN work_items w ON w.work_id=r.local_work_id JOIN journal_events je ON je.event_id=r.acceptance_event_id JOIN work_item_inputs qi ON qi.work_id=r.local_work_id AND qi.relationship='trigger' JOIN journal_events qj ON qj.journal_offset=r.journal_offset AND qj.work_id=r.local_work_id AND qj.event_type='work.queued'").fetch_all(&self.runtime.inner.pool).await.map_err(storage)?;
        let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM distribution_receipts")
            .fetch_one(&self.runtime.inner.pool)
            .await
            .map_err(storage)?;
        if count != rows.len() as i64 {
            return Err(invalid());
        }
        for r in rows {
            super::codec::decode_message_row(&r).map_err(storage)?;
            let receipt = parse(
                &r.try_get::<String, _>("receipt_canonical")
                    .map_err(storage)?,
            )?;
            let material = parse(
                &r.try_get::<String, _>("material_canonical")
                    .map_err(storage)?,
            )?;
            let event = parse(&r.try_get::<String, _>("payload_json").map_err(storage)?)?;
            for (wire, column) in [
                ("work_id", "central_work_id"),
                ("receipt_id", "receipt_id"),
                ("local_work_id", "local_work_id"),
                ("local_message_id", "local_message_id"),
                ("local_conversation_id", "local_conversation_id"),
                ("material_digest", "material_digest"),
                ("admitted_at", "admitted_at"),
                ("origin_installation_id", "origin_installation_id"),
            ] {
                if text(&receipt, wire)? != r.try_get::<String, _>(column).map_err(storage)? {
                    return Err(invalid());
                }
            }
            for (field, column) in [
                ("conversation_sequence", "central_sequence"),
                ("local_work_ordinal", "local_work_ordinal"),
                ("journal_offset", "journal_offset"),
                ("origin_generation", "origin_generation"),
            ] {
                if receipt[field] != r.try_get::<i64, _>(column).map_err(storage)?.to_string() {
                    return Err(invalid());
                }
            }
            if material["customer_id"] != r.try_get::<String, _>("customer_id").map_err(storage)?
                || material["logical_agent_id"]
                    != r.try_get::<String, _>("logical_agent_id")
                        .map_err(storage)?
                || material["work_id"] != receipt["work_id"]
                || material["conversation_sequence"] != receipt["conversation_sequence"]
                || receipt["admitted_at"]
                    != r.try_get::<String, _>("committed_at").map_err(storage)?
            {
                return Err(invalid());
            }
            if event["origin"]["type"] != "distribution"
                || event["origin"]["receipt_id"] != receipt["receipt_id"]
                || event["message_id"] != receipt["local_message_id"]
                || event["content"] != material["content"]
                || r.try_get::<i64, _>("event_version").map_err(storage)? != 3
                || r.try_get::<String, _>("event_type").map_err(storage)? != "message.accepted"
                || r.try_get::<String, _>("input_event_id").map_err(storage)?
                    != r.try_get::<String, _>("acceptance_event_id")
                        .map_err(storage)?
                || r.try_get::<Option<String>, _>("reply_binding_id")
                    .map_err(storage)?
                    .is_some()
                || receipt["local_work_ordinal"]
                    != r.try_get::<i64, _>("conversation_work_ordinal")
                        .map_err(storage)?
                        .to_string()
                || r.try_get::<String, _>("distribution_receipt_id")
                    .map_err(storage)?
                    != text(&receipt, "receipt_id")?
                || digest_material(DigestDomain::Work, &material).map_err(|_| invalid())?
                    != text(&receipt, "material_digest")?
            {
                return Err(invalid());
            }
        }
        let contradictions=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM distribution_controls c JOIN distribution_receipts r ON r.environment=c.environment AND r.central_work_id=c.central_work_id WHERE c.local_work_id IS NULL OR c.local_work_id<>r.local_work_id OR c.material_digest<>r.material_digest OR c.customer_id<>r.customer_id OR c.logical_agent_id<>r.logical_agent_id").fetch_one(&self.runtime.inner.pool).await.map_err(storage)?;
        if contradictions != 0 {
            return Err(invalid());
        }
        let controls = sqlx::query("SELECT * FROM distribution_controls")
            .fetch_all(&self.runtime.inner.pool)
            .await
            .map_err(storage)?;
        for row in controls {
            let c = parse(
                &row.try_get::<String, _>("control_canonical")
                    .map_err(storage)?,
            )?;
            let receipt = parse(
                &row.try_get::<String, _>("receipt_canonical")
                    .map_err(storage)?,
            )?;
            if digest_material(DigestDomain::Cancel, &c).map_err(|_| invalid())?
                != row
                    .try_get::<String, _>("control_digest")
                    .map_err(storage)?
                || c["control_id"] != receipt["control_id"]
                || c["work_id"] != receipt["work_id"]
                || c["work_id"]
                    != row
                        .try_get::<String, _>("central_work_id")
                        .map_err(storage)?
                || c["customer_id"] != row.try_get::<String, _>("customer_id").map_err(storage)?
                || c["logical_agent_id"]
                    != row
                        .try_get::<String, _>("logical_agent_id")
                        .map_err(storage)?
                || c["material_digest"]
                    != row
                        .try_get::<String, _>("material_digest")
                        .map_err(storage)?
            {
                return Err(invalid());
            }
            let local = row
                .try_get::<Option<String>, _>("local_work_id")
                .map_err(storage)?;
            if receipt["outcome"] == "cancelled_before_admission" {
                if local.is_some()
                    || row
                        .try_get::<Option<String>, _>("cancellation_event_id")
                        .map_err(storage)?
                        .is_some()
                {
                    return Err(invalid());
                }
                if row
                    .try_get::<i64, _>("sequence_reconciled")
                    .map_err(storage)?
                    == 1
                {
                    let material = parse(
                        &row.try_get::<String, _>("target_material_canonical")
                            .map_err(storage)?,
                    )?;
                    if digest_material(DigestDomain::Work, &material).map_err(|_| invalid())?
                        != text(&c, "material_digest")?
                        || material["work_id"] != c["work_id"]
                        || material["conversation_id"]
                            != row
                                .try_get::<String, _>("central_conversation_id")
                                .map_err(storage)?
                        || material["conversation_sequence"]
                            != row
                                .try_get::<i64, _>("central_sequence")
                                .map_err(storage)?
                                .to_string()
                    {
                        return Err(invalid());
                    }
                }
            } else if local.as_deref() != receipt["local_work_id"].as_str() {
                return Err(invalid());
            }
            if let Some(event_id) = row
                .try_get::<Option<String>, _>("cancellation_event_id")
                .map_err(storage)?
            {
                let e = sqlx::query(
                    "SELECT event_version,payload_json FROM journal_events WHERE event_id=?",
                )
                .bind(event_id)
                .fetch_one(&self.runtime.inner.pool)
                .await
                .map_err(storage)?;
                let p = parse(&e.try_get::<String, _>("payload_json").map_err(storage)?)?;
                if e.try_get::<i64, _>("event_version").map_err(storage)? != 3
                    || p["control_id"] != c["control_id"]
                    || p["transition"]["work_id"].as_str() != local.as_deref()
                {
                    return Err(invalid());
                }
            }
        }
        let conversations = sqlx::query(
            "SELECT central_conversation_id,next_sequence FROM distribution_conversations",
        )
        .fetch_all(&self.runtime.inner.pool)
        .await
        .map_err(storage)?;
        for c in conversations {
            let id: String = c.try_get("central_conversation_id").map_err(storage)?;
            let rows=sqlx::query_scalar::<_,i64>("SELECT central_sequence FROM distribution_receipts WHERE central_conversation_id=? UNION SELECT central_sequence FROM distribution_controls WHERE central_conversation_id=? AND sequence_reconciled=1 AND local_work_id IS NULL ORDER BY central_sequence").bind(&id).bind(&id).fetch_all(&self.runtime.inner.pool).await.map_err(storage)?;
            if c.try_get::<i64, _>("next_sequence").map_err(storage)? != rows.len() as i64 + 1
                || rows.iter().enumerate().any(|(i, n)| *n != i as i64 + 1)
            {
                return Err(DistributionError::Reconciliation);
            }
        }
        Ok(())
    }
}
