# Repository Guidelines

Never commit credentials, local databases/WAL files, artifacts, test evidence containing secrets, Terraform state, or build output. Redact tokens, content, commands, outputs, and headers from logs and review artifacts by default.

## Validation efficiency

Optimize for meaningful verification, not repetitive ceremony.

During implementation, run only focused checks that can catch regressions in the subsystem being changed. Do not repeatedly run repository-wide checks, schema inventories, Git-state audits, documentation scans, checker probe suites, or unchanged-stage regressions after each small edit.

At the end of a stage, use at most three validation layers:

1. Focused tests while implementing.
2. One consolidated repository-wide gate after the stage is complete.
3. One independent verification pass against the stage contract.

Do not verify the verifier. Do not rerun a passing full gate unless subsequent code changes could invalidate it. Do not repeat old-stage validation unless the current change touches those invariants.

Treat `scripts/check-public-repository.mjs`, `scripts/verify`, full Cargo test matrices, cargo-deny, schema/fingerprint audits, and similar expensive checks as final gates unless the current task specifically changes those systems.

Avoid long polling loops and repetitive progress narration for quiet commands. Wait in bounded intervals and report only meaningful state changes, failures, or final results.

Behavioral tests are authoritative for runtime behavior. Repository checkers should enforce straightforward structural policy only; do not build mutation-test or static-analysis machinery to prove arbitrary malicious source rewrites unless that is explicitly the purpose of the stage.

When a focused check already proves a fact, reuse that result rather than proving the same fact through another equivalent command.
