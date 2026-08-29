//! Transport-neutral mutation admission and committed-cursor notification.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::application::command_service::CommandPostCommit;
use crate::application::scheduler::SchedulerNotifier;
use crate::domain::{JournalOffset, WorkId};
use crate::protocol::{CURSOR_BROADCAST_CAPACITY, ReplayCursor};

#[derive(Clone)]
pub struct MutationAdmission {
    accepting: Arc<AtomicBool>,
    gate: Arc<tokio::sync::RwLock<()>>,
}

impl Default for MutationAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationAdmission {
    #[must_use]
    pub fn new() -> Self {
        Self {
            accepting: Arc::new(AtomicBool::new(true)),
            gate: Arc::new(tokio::sync::RwLock::new(())),
        }
    }

    pub async fn admit(&self) -> Result<MutationPermit, AdmissionClosed> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(AdmissionClosed);
        }
        let guard = Arc::clone(&self.gate).read_owned().await;
        if !self.accepting.load(Ordering::Acquire) {
            drop(guard);
            return Err(AdmissionClosed);
        }
        Ok(MutationPermit { _guard: guard })
    }

    pub async fn close_and_wait(&self) {
        self.accepting.store(false, Ordering::Release);
        let quiesced = Arc::clone(&self.gate).write_owned().await;
        drop(quiesced);
    }

    #[must_use]
    pub fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::Acquire)
    }
}

pub struct MutationPermit {
    _guard: tokio::sync::OwnedRwLockReadGuard<()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmissionClosed;

#[derive(Clone)]
pub struct CursorBroadcaster {
    sender: tokio::sync::broadcast::Sender<ReplayCursor>,
}

impl Default for CursorBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorBroadcaster {
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(CURSOR_BROADCAST_CAPACITY);
        Self { sender }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ReplayCursor> {
        self.sender.subscribe()
    }

    pub fn publish(&self, cursor: JournalOffset) {
        let _ = self.sender.send(ReplayCursor::from_journal_offset(cursor));
    }
}

#[derive(Clone)]
pub struct CommandCommitEffects {
    cursors: CursorBroadcaster,
    scheduler: Option<SchedulerNotifier>,
}

impl CommandCommitEffects {
    #[must_use]
    pub const fn new(cursors: CursorBroadcaster, scheduler: Option<SchedulerNotifier>) -> Self {
        Self { cursors, scheduler }
    }

    fn wake_scheduler(&self) {
        if let Some(scheduler) = &self.scheduler {
            scheduler.wake();
        }
    }
}

impl CommandPostCommit for CommandCommitEffects {
    fn message_committed(&self, _: WorkId, cursor: JournalOffset) {
        self.cursors.publish(cursor);
        self.wake_scheduler();
    }

    fn active_cancellation_committed(&self, _: WorkId, cursor: JournalOffset) {
        self.cursors.publish(cursor);
        self.wake_scheduler();
    }

    fn direct_cancellation_committed(&self, _: WorkId, cursor: JournalOffset) {
        self.cursors.publish(cursor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admission_close_waits_for_inflight_and_permanently_rejects_new_work() {
        let admission = MutationAdmission::new();
        let permit = admission.admit().await.unwrap();
        let closer = admission.clone();
        let mut closed = tokio::spawn(async move {
            closer.close_and_wait().await;
        });
        tokio::task::yield_now().await;
        assert!(!closed.is_finished());
        assert!(admission.admit().await.is_err());
        drop(permit);
        (&mut closed).await.unwrap();
        assert!(admission.admit().await.is_err());
    }

    #[tokio::test]
    async fn cursor_broadcast_is_a_bounded_committed_hint() {
        let broadcaster = CursorBroadcaster::new();
        let mut receiver = broadcaster.subscribe();
        for cursor in 1..=CURSOR_BROADCAST_CAPACITY + 1 {
            broadcaster.publish(JournalOffset::try_new(cursor as i64).unwrap());
        }
        assert!(matches!(
            receiver.recv().await,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(1))
        ));
        assert_eq!(receiver.recv().await.unwrap().get(), 2);
    }
}
