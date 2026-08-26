# Development workflow

The baseline local verification command for normal repository changes is:

```sh
scripts/verify
```

It resolves the repository root, supplies the documented local tool paths, and runs
all currently mandatory format, compile, lint, test, dependency-tree, structured
repository, Markdown, and renderer-syntax checks. A failed mandatory check stops the
gate and returns nonzero. Mandatory checks are never silently skipped.

## Test result semantics

Future environment-specific runners may report `PASS`, `FAIL`, `NOT_CONFIGURED`, or
`NOT_RUN`. `PASS` means the requested suite actually executed successfully.
`NOT_CONFIGURED` means a required tool, credential, service, capability, or target
configuration is absent. `NOT_RUN` means the suite did not execute for another stated
reason, such as running on the wrong operating system.

When an environment-specific suite is explicitly requested but cannot run, it must
never be represented as `PASS`. It reports `NOT_CONFIGURED` or `NOT_RUN` with a reason,
and returns nonzero whenever that suite is mandatory for the invoked local, merge, or
release gate. Ignored tests alone are not evidence that a suite ran or passed.

## Test classes

The named invocations below describe future organization, not commands or suites that
exist today. A runner may introduce a stable `scripts/test <profile>` interface when
the first non-portable suite is implemented.

| Profile | Intended future organization | Execution environment | Mandatory once implemented | Missing prerequisites |
| --- | --- | --- | --- | --- |
| `portable` | Portable unit, contract, and process-free integration tests; currently represented by the Cargo tests inside `scripts/verify` | Supported macOS and Linux development hosts | Local, merge, and release | `NOT_CONFIGURED` with the missing toolchain or capability; mandatory gates return nonzero |
| `sqlite-file` | File-backed SQLite migration, reopen, WAL, locking, and persistence tests in isolated temporary directories | Supported macOS and Linux filesystems | Local, merge, and release | `NOT_CONFIGURED` with the unavailable filesystem or SQLite capability; mandatory gates return nonzero |
| `ubuntu-workstation` | Linux workstation, process-tree, Unix, cgroup, and target service semantics | Validated Ubuntu 24.04 target hosts | Merge and release; local only when that target is explicitly requested | `NOT_RUN` on the wrong OS or `NOT_CONFIGURED` for missing host capabilities; requested mandatory gates return nonzero |
| `live-provider` | Provider-neutral live adapter acceptance against explicitly configured accounts and endpoints | Isolated, credentialed release-live runner or deliberate local opt-in | Not routine local or merge; mandatory for the release-live gate when invoked | `NOT_CONFIGURED` with a redacted reason when credentials or endpoint configuration are absent; a required release-live gate returns nonzero |
| `crash-disposable` | Destructive crash, restart, signal, recovery, and orphan-cleanup scenarios | Explicit disposable Ubuntu target hosts | Merge and release on the designated job; local only by explicit opt-in | `NOT_RUN` off a disposable target or `NOT_CONFIGURED` for missing privileges/capabilities; mandatory gates return nonzero |
| `macos-client` | Native Swift unit, UI, protocol-fixture, Keychain, lifecycle, and reconnect suites | Validated macOS runners with the required Xcode toolchain | Local on macOS, merge, and release | `NOT_RUN` off macOS or `NOT_CONFIGURED` for missing Xcode/client prerequisites; mandatory gates return nonzero |

## Future CI shape

The provider-neutral future job shapes are:

- `macos`: baseline, portable, SQLite-file, and macOS-client checks on validated macOS;
- `ubuntu-24.04`: baseline, portable, and SQLite-file checks on Ubuntu 24.04;
- `ubuntu-target-host`: workstation and crash-disposable suites on an explicit target;
- `release-live`: deliberately authorized live-provider acceptance with protected
  credentials and redacted evidence.

No CI provider configuration exists yet. The GitHub-looking `origin` has not been
externally validated, so CI YAML is added only after remote and runner semantics are
validated. Privileged systemd, cgroup, sudo, and process-tree tests belong on explicit
target hosts; they must not be weakened to create passing results on generic runners.
