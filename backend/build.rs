use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );
    emit_git_rerun_inputs(&manifest_dir);

    let (git_revision, git_dirty) = git_metadata(&manifest_dir);
    let package_version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();
    let build_timestamp_epoch = source_date_epoch();

    println!("cargo:rustc-env=CRAXII_PACKAGE_VERSION={package_version}");
    println!("cargo:rustc-env=CRAXII_GIT_REVISION={git_revision}");
    println!("cargo:rustc-env=CRAXII_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=CRAXII_BUILD_TARGET={target}");
    println!("cargo:rustc-env=CRAXII_BUILD_TIMESTAMP_EPOCH={build_timestamp_epoch}");
}

fn source_date_epoch() -> String {
    match env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => {
            let normalized = value.trim();
            if normalized.parse::<u64>().is_ok() {
                normalized.to_owned()
            } else {
                "invalid".to_owned()
            }
        }
        Err(env::VarError::NotPresent) => String::new(),
        Err(env::VarError::NotUnicode(_)) => "invalid".to_owned(),
    }
}

fn git_metadata(manifest_dir: &Path) -> (String, bool) {
    let revision = git(manifest_dir, &["rev-parse", "--verify", "HEAD"])
        .and_then(|value| normalize_revision(&value));
    let status = git(
        manifest_dir,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    );

    match (revision, status) {
        (Some(revision), Some(status)) => (revision, !status.is_empty()),
        _ => ("unversioned".to_owned(), true),
    }
}

fn normalize_revision(value: &str) -> Option<String> {
    let value = value.trim();
    if (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

fn git(manifest_dir: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn emit_git_rerun_inputs(manifest_dir: &Path) {
    let Some(repository_root) = git_path(manifest_dir, &["rev-parse", "--show-toplevel"]) else {
        return;
    };
    emit_rerun_path(&repository_root);

    if let Some(git_dir) = git_path(manifest_dir, &["rev-parse", "--absolute-git-dir"]) {
        emit_rerun_path(&git_dir);
    }
    if let Some(common_dir) = git_path(
        manifest_dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        emit_rerun_path(&common_dir);
    }
}

fn git_path(manifest_dir: &Path, arguments: &[&str]) -> Option<PathBuf> {
    git(manifest_dir, arguments)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn emit_rerun_path(path: &Path) {
    if path.exists()
        && let Some(path) = path.to_str()
    {
        println!("cargo:rerun-if-changed={path}");
    }
}
