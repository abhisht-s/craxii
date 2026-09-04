use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::bootstrap::secret::SecretString;

const SYSTEMD_CREDENTIAL_DIRECTORY: &str = "/run/credentials/craxii";
const MAX_CREDENTIAL_BYTES: u64 = 16 * 1024;

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

#[derive(Clone, Eq, PartialEq)]
pub enum CredentialSourceConfig {
    LocalDirectory { directory: PathBuf },
    Systemd,
}

impl Debug for CredentialSourceConfig {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalDirectory { .. } => {
                formatter.write_str("CredentialSourceConfig::LocalDirectory([REDACTED])")
            }
            Self::Systemd => formatter.write_str("CredentialSourceConfig::Systemd"),
        }
    }
}

impl CredentialSourceConfig {
    pub fn local_directory(&self) -> Option<&Path> {
        match self {
            Self::LocalDirectory { directory } => Some(directory),
            Self::Systemd => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialLoadErrorKind {
    Missing,
    UnsafeFile,
    Oversized,
    InvalidValue,
    Storage,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CredentialLoadError(CredentialLoadErrorKind);

impl CredentialLoadError {
    #[must_use]
    pub const fn kind(self) -> CredentialLoadErrorKind {
        self.0
    }
}

impl Display for CredentialLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("provider credential unavailable")
    }
}

impl Debug for CredentialLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialLoadError")
            .field("kind", &self.0)
            .finish()
    }
}

impl std::error::Error for CredentialLoadError {}

/// Loads only explicitly referenced logical credentials, once, into nonprintable wrappers.
pub fn load_credentials<'a>(
    source: &CredentialSourceConfig,
    references: impl IntoIterator<Item = &'a CredentialRef>,
) -> Result<BTreeMap<String, SecretString>, CredentialLoadError> {
    let directory = match source {
        CredentialSourceConfig::LocalDirectory { directory } => directory.as_path(),
        CredentialSourceConfig::Systemd => Path::new(SYSTEMD_CREDENTIAL_DIRECTORY),
    };
    let directory_metadata = std::fs::symlink_metadata(directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CredentialLoadError(CredentialLoadErrorKind::Missing)
        } else {
            CredentialLoadError(CredentialLoadErrorKind::Storage)
        }
    })?;
    if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
        return Err(CredentialLoadError(CredentialLoadErrorKind::UnsafeFile));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if directory_metadata.mode() & 0o022 != 0 {
            return Err(CredentialLoadError(CredentialLoadErrorKind::UnsafeFile));
        }
    }
    let mut loaded = BTreeMap::new();
    for reference in references {
        if loaded.contains_key(reference.as_str()) {
            continue;
        }
        let path = directory.join(reference.as_str());
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CredentialLoadError(CredentialLoadErrorKind::Missing)
            } else {
                CredentialLoadError(CredentialLoadErrorKind::Storage)
            }
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(CredentialLoadError(CredentialLoadErrorKind::UnsafeFile));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o077 != 0
                || metadata.nlink() != 1
                || metadata.uid() != directory_metadata.uid()
            {
                return Err(CredentialLoadError(CredentialLoadErrorKind::UnsafeFile));
            }
        }
        if metadata.len() == 0 || metadata.len() > MAX_CREDENTIAL_BYTES {
            return Err(CredentialLoadError(if metadata.len() == 0 {
                CredentialLoadErrorKind::InvalidValue
            } else {
                CredentialLoadErrorKind::Oversized
            }));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|_| CredentialLoadError(CredentialLoadErrorKind::Storage))?;
        let opened = file
            .metadata()
            .map_err(|_| CredentialLoadError(CredentialLoadErrorKind::Storage))?;
        if !opened.is_file() || opened.len() != metadata.len() {
            return Err(CredentialLoadError(CredentialLoadErrorKind::UnsafeFile));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if opened.dev() != metadata.dev()
                || opened.ino() != metadata.ino()
                || opened.mode() & 0o077 != 0
                || opened.nlink() != 1
                || opened.uid() != directory_metadata.uid()
            {
                return Err(CredentialLoadError(CredentialLoadErrorKind::UnsafeFile));
            }
        }
        let mut bytes = Vec::new();
        file.take(MAX_CREDENTIAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| CredentialLoadError(CredentialLoadErrorKind::Storage))?;
        if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
            return Err(CredentialLoadError(CredentialLoadErrorKind::Oversized));
        }
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        let value = String::from_utf8(bytes)
            .map_err(|_| CredentialLoadError(CredentialLoadErrorKind::InvalidValue))?;
        if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
            return Err(CredentialLoadError(CredentialLoadErrorKind::InvalidValue));
        }
        loaded.insert(reference.as_str().to_owned(), SecretString::new(value));
    }
    if loaded.is_empty() {
        return Err(CredentialLoadError(CredentialLoadErrorKind::Missing));
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "craxii-credential-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn local_credential_is_loaded_trimmed_and_never_formatted() {
        let directory = root();
        let path = directory.join("openai_primary");
        fs::write(&path, "sentinel-api-key\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let reference = CredentialRef::new("openai_primary".to_owned());
        let loaded = load_credentials(
            &CredentialSourceConfig::LocalDirectory {
                directory: directory.clone(),
            },
            [&reference],
        )
        .unwrap();
        let secret = loaded.get("openai_primary").unwrap();
        assert_eq!(secret.expose_secret(), "sentinel-api-key");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn symlink_permissive_empty_and_whitespace_credentials_fail_closed() {
        for (name, contents, mode, expected) in [
            (
                "permissive",
                "key",
                0o644,
                CredentialLoadErrorKind::UnsafeFile,
            ),
            ("empty", "", 0o600, CredentialLoadErrorKind::InvalidValue),
            (
                "whitespace",
                " key",
                0o600,
                CredentialLoadErrorKind::InvalidValue,
            ),
        ] {
            let directory = root();
            let path = directory.join(name);
            fs::write(&path, contents).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            let reference = CredentialRef::new(name.to_owned());
            assert_eq!(
                load_credentials(
                    &CredentialSourceConfig::LocalDirectory {
                        directory: directory.clone(),
                    },
                    [&reference],
                )
                .unwrap_err()
                .kind(),
                expected
            );
            fs::remove_dir_all(directory).unwrap();
        }

        let directory = root();
        fs::write(directory.join("target"), "key").unwrap();
        fs::set_permissions(directory.join("target"), fs::Permissions::from_mode(0o600)).unwrap();
        symlink(directory.join("target"), directory.join("linked")).unwrap();
        let reference = CredentialRef::new("linked".to_owned());
        assert_eq!(
            load_credentials(
                &CredentialSourceConfig::LocalDirectory {
                    directory: directory.clone(),
                },
                [&reference],
            )
            .unwrap_err()
            .kind(),
            CredentialLoadErrorKind::UnsafeFile
        );
        fs::remove_dir_all(directory).unwrap();

        let directory = root();
        let actual = directory.join("actual");
        fs::create_dir(&actual).unwrap();
        let credential = actual.join("key");
        fs::write(&credential, "key").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        let linked = directory.join("linked-directory");
        symlink(&actual, &linked).unwrap();
        let reference = CredentialRef::new("key".to_owned());
        assert_eq!(
            load_credentials(
                &CredentialSourceConfig::LocalDirectory { directory: linked },
                [&reference],
            )
            .unwrap_err()
            .kind(),
            CredentialLoadErrorKind::UnsafeFile
        );
        fs::remove_dir_all(directory).unwrap();

        let directory = root();
        let credential = directory.join("key");
        fs::write(&credential, "key").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
        let reference = CredentialRef::new("key".to_owned());
        assert_eq!(
            load_credentials(
                &CredentialSourceConfig::LocalDirectory {
                    directory: directory.clone(),
                },
                [&reference],
            )
            .unwrap_err()
            .kind(),
            CredentialLoadErrorKind::UnsafeFile
        );
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_malformed_and_nonprefixed_credentials_have_redacted_outcomes() {
        let directory = root();
        let missing = CredentialRef::new("missing".to_owned());
        let error = load_credentials(
            &CredentialSourceConfig::LocalDirectory {
                directory: directory.clone(),
            },
            [&missing],
        )
        .unwrap_err();
        assert_eq!(error.kind(), CredentialLoadErrorKind::Missing);
        assert_eq!(error.to_string(), "provider credential unavailable");
        assert!(!format!("{error:?}").contains(directory.to_str().unwrap()));

        let invalid = directory.join("invalid");
        fs::write(&invalid, [0xff, 0xfe]).unwrap();
        fs::set_permissions(&invalid, fs::Permissions::from_mode(0o600)).unwrap();
        let reference = CredentialRef::new("invalid".to_owned());
        assert_eq!(
            load_credentials(
                &CredentialSourceConfig::LocalDirectory {
                    directory: directory.clone(),
                },
                [&reference],
            )
            .unwrap_err()
            .kind(),
            CredentialLoadErrorKind::InvalidValue
        );

        let nonprefixed = directory.join("nonprefixed");
        fs::write(&nonprefixed, "provider key without assumed prefix\n").unwrap();
        fs::set_permissions(&nonprefixed, fs::Permissions::from_mode(0o600)).unwrap();
        let reference = CredentialRef::new("nonprefixed".to_owned());
        let loaded = load_credentials(
            &CredentialSourceConfig::LocalDirectory {
                directory: directory.clone(),
            },
            [&reference],
        )
        .unwrap();
        assert_eq!(
            loaded.get("nonprefixed").unwrap().expose_secret(),
            "provider key without assumed prefix"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
