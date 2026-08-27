use std::path::{Path, PathBuf};

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialRef(String);

impl CredentialRef {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CredentialRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("CredentialRef")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialSourceConfig {
    LocalDirectory { directory: PathBuf },
    Systemd,
}

impl CredentialSourceConfig {
    pub fn local_directory(&self) -> Option<&Path> {
        match self {
            Self::LocalDirectory { directory } => Some(directory),
            Self::Systemd => None,
        }
    }
}
