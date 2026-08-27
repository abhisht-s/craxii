use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::ProcessMetadata;

#[derive(Debug)]
pub struct ApplicationShell {
    process_metadata: ProcessMetadata,
    health: Health,
}

impl ApplicationShell {
    pub(crate) fn new(process_metadata: ProcessMetadata, health: Health) -> Self {
        Self {
            process_metadata,
            health,
        }
    }

    pub fn process_metadata(&self) -> &ProcessMetadata {
        &self.process_metadata
    }

    pub fn health(&self) -> &Health {
        &self.health
    }
}
