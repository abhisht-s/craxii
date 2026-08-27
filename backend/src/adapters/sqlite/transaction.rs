use std::time::Instant;

use sqlx::{Sqlite, SqliteConnection, Transaction};
use tokio::sync::OwnedMutexGuard;

use super::error::SqliteAdapterError;
use super::runtime::SqliteRuntime;

/// Adapter-private `BEGIN IMMEDIATE` scope. It never crosses the SQLite module boundary.
pub(super) struct WriteTransaction {
    coordinator: Option<OwnedMutexGuard<()>>,
    transaction: Option<Transaction<'static, Sqlite>>,
    intent: &'static str,
    started: Instant,
}

impl WriteTransaction {
    pub(super) async fn begin(
        runtime: &SqliteRuntime,
        intent: &'static str,
    ) -> Result<Self, SqliteAdapterError> {
        let coordinator = runtime.inner.write_coordinator.clone().lock_owned().await;
        let started = Instant::now();
        let transaction = runtime.inner.pool.begin_with("BEGIN IMMEDIATE").await;
        let transaction = match transaction {
            Ok(transaction) => transaction,
            Err(error) => {
                let classified = SqliteAdapterError::from_sqlx(error);
                tracing::warn!(
                    target: "craxii::sqlite",
                    operation = "transaction_begin",
                    intent,
                    mode = "immediate",
                    outcome = "error",
                    category = ?classified.kind(),
                    sqlite_code = ?classified.sqlite_code()
                );
                return Err(classified);
            }
        };
        tracing::debug!(
            target: "craxii::sqlite",
            operation = "transaction_begin",
            intent,
            mode = "immediate",
            outcome = "ok"
        );
        Ok(Self {
            coordinator: Some(coordinator),
            transaction: Some(transaction),
            intent,
            started,
        })
    }

    /// Primitive for named SQLite adapter methods only; no application callback is accepted.
    pub(super) fn connection(&mut self) -> &mut SqliteConnection {
        self.transaction
            .as_mut()
            .expect("unfinished transaction retains its connection")
    }

    pub(super) async fn commit(mut self) -> Result<(), SqliteAdapterError> {
        let result = self
            .transaction
            .take()
            .expect("unfinished transaction retains its transaction")
            .commit()
            .await
            .map_err(SqliteAdapterError::from_sqlx);
        trace_finish(self.intent, "commit", self.started, result.as_ref().err());
        if result.is_ok() {
            self.coordinator.take();
        }
        result
    }

    pub(super) async fn rollback(mut self) -> Result<(), SqliteAdapterError> {
        let result = self
            .transaction
            .take()
            .expect("unfinished transaction retains its transaction")
            .rollback()
            .await
            .map_err(SqliteAdapterError::from_sqlx);
        trace_finish(self.intent, "rollback", self.started, result.as_ref().err());
        if result.is_ok() {
            self.coordinator.take();
        }
        result
    }
}

fn trace_finish(
    intent: &'static str,
    action: &'static str,
    started: Instant,
    error: Option<&SqliteAdapterError>,
) {
    tracing::debug!(
        target: "craxii::sqlite",
        operation = "transaction_finish",
        intent,
        action,
        outcome = if error.is_some() { "error" } else { "ok" },
        category = ?error.map(|value| value.kind()),
        sqlite_code = ?error.and_then(|value| value.sqlite_code()),
        duration_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
    );
}

impl Drop for WriteTransaction {
    fn drop(&mut self) {
        if self.transaction.is_some() {
            tracing::debug!(
                target: "craxii::sqlite",
                operation = "transaction_finish",
                intent = self.intent,
                action = "drop_rollback",
                outcome = "queued"
            );
        }
    }
}
