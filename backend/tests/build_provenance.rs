use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const BUILD_SCRIPT: &str = include_str!("../build.rs");
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn cargo_rebuilds_truthful_repository_wide_dirty_provenance() {
    let repository = TestRepository::new();

    let clean = repository.build_metadata();
    assert!(!clean.dirty);
    repository.assert_all_rerun_paths_exist();

    fs::write(repository.path().join("AGENTS.md"), "tracked dirty\n").unwrap();
    assert!(repository.build_metadata().dirty);
    repository.git(&["restore", "AGENTS.md"]);
    assert!(!repository.build_metadata().dirty);

    fs::write(repository.path().join("AGENTS.md"), "staged dirty\n").unwrap();
    repository.git(&["add", "AGENTS.md"]);
    assert!(repository.build_metadata().dirty);
    repository.git(&["restore", "--staged", "AGENTS.md"]);
    repository.git(&["restore", "AGENTS.md"]);
    assert!(!repository.build_metadata().dirty);

    let untracked = repository.path().join("root-untracked.txt");
    fs::write(&untracked, "untracked dirty\n").unwrap();
    assert!(repository.build_metadata().dirty);
    fs::remove_file(untracked).unwrap();
    assert!(!repository.build_metadata().dirty);
}

#[test]
fn linked_worktree_uses_its_git_state_and_common_metadata() {
    let repository = TestRepository::new();
    let linked = repository.container.join("linked");
    repository.git(&[
        "worktree",
        "add",
        "-b",
        "linked-provenance-probe",
        linked.to_str().unwrap(),
        "HEAD",
    ]);
    assert!(linked.join(".git").is_file());

    let clean = TestRepository::build_metadata_at(&linked);
    assert!(!clean.dirty);

    fs::write(linked.join("AGENTS.md"), "linked tracked dirty\n").unwrap();
    let dirty = TestRepository::build_metadata_at(&linked);
    assert!(dirty.dirty);
    assert_eq!(dirty.revision, clean.revision);

    TestRepository::git_at(&linked, &["add", "AGENTS.md"]);
    TestRepository::git_at(&linked, &["commit", "-m", "linked probe"]);
    let committed = TestRepository::build_metadata_at(&linked);
    assert!(!committed.dirty);
    assert_ne!(committed.revision, clean.revision);
}

#[test]
fn unavailable_git_metadata_falls_back_to_unversioned_and_dirty() {
    let repository = TestRepository::new();
    fs::remove_dir_all(repository.path().join(".git")).unwrap();

    assert_eq!(
        repository.build_metadata(),
        EmbeddedMetadata {
            revision: "unversioned".to_owned(),
            dirty: true,
        }
    );
}

#[derive(Debug, Eq, PartialEq)]
struct EmbeddedMetadata {
    revision: String,
    dirty: bool,
}

struct TestRepository {
    container: PathBuf,
    repository: PathBuf,
}

impl TestRepository {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let container = std::env::temp_dir().join(format!(
            "craxii-build-provenance-test-{}-{sequence}",
            std::process::id()
        ));
        let repository = container.join("repository");
        fs::create_dir_all(repository.join("backend/src")).unwrap();
        fs::write(
            repository.join("Cargo.toml"),
            "[workspace]\nmembers = [\"backend\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        fs::write(repository.join(".gitignore"), "target/\n").unwrap();
        fs::write(repository.join("AGENTS.md"), "clean\n").unwrap();
        fs::write(
            repository.join("backend/Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"provenance-probe\"\n",
                "version = \"0.0.1\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n",
            ),
        )
        .unwrap();
        fs::write(repository.join("backend/build.rs"), BUILD_SCRIPT).unwrap();
        fs::write(
            repository.join("backend/src/main.rs"),
            concat!(
                "fn main() {\n",
                "    println!(\"{} {}\", env!(\"CRAXII_GIT_REVISION\"), ",
                "env!(\"CRAXII_GIT_DIRTY\"));\n",
                "}\n",
            ),
        )
        .unwrap();

        Self::git_at(&repository, &["init", "--quiet"]);
        Self::git_at(&repository, &["config", "user.name", "Craxii Test"]);
        Self::git_at(
            &repository,
            &["config", "user.email", "craxii-test@example.invalid"],
        );
        let output = Command::new(env!("CARGO"))
            .arg("generate-lockfile")
            .current_dir(&repository)
            .output()
            .unwrap();
        assert_success("cargo generate-lockfile", &output);
        Self::git_at(&repository, &["add", "."]);
        Self::git_at(&repository, &["commit", "--quiet", "-m", "fixture"]);

        Self {
            container,
            repository,
        }
    }

    fn path(&self) -> &Path {
        &self.repository
    }

    fn git(&self, arguments: &[&str]) {
        Self::git_at(&self.repository, arguments);
    }

    fn git_at(repository: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .output()
            .unwrap();
        assert_success("git", &output);
    }

    fn build_metadata(&self) -> EmbeddedMetadata {
        Self::build_metadata_at(&self.repository)
    }

    fn build_metadata_at(repository: &Path) -> EmbeddedMetadata {
        let output = Command::new(env!("CARGO"))
            .arg("build")
            .arg("--quiet")
            .current_dir(repository)
            .output()
            .unwrap();
        assert_success("cargo build", &output);

        let output = Command::new(repository.join("target/debug/provenance-probe"))
            .output()
            .unwrap();
        assert_success("provenance probe", &output);
        let stdout = String::from_utf8(output.stdout).unwrap();
        let (revision, dirty) = stdout.trim().split_once(' ').unwrap();
        EmbeddedMetadata {
            revision: revision.to_owned(),
            dirty: dirty.parse().unwrap(),
        }
    }

    fn assert_all_rerun_paths_exist(&self) {
        let build_root = self.repository.join("target/debug/build");
        let outputs: Vec<_> = fs::read_dir(build_root)
            .unwrap()
            .map(|entry| entry.unwrap().path().join("output"))
            .filter(|path| path.is_file())
            .collect();
        assert_eq!(
            outputs.len(),
            1,
            "unexpected build-script outputs: {outputs:?}"
        );

        let output = fs::read_to_string(&outputs[0]).unwrap();
        let paths: Vec<_> = output
            .lines()
            .filter_map(|line| line.strip_prefix("cargo:rerun-if-changed="))
            .collect();
        assert!(!paths.is_empty());
        for path in paths {
            assert!(Path::new(path).exists(), "watched path is missing: {path}");
        }
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.container);
    }
}

fn assert_success(operation: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
