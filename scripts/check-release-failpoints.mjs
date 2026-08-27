#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = join(scriptDir, '..');
const releaseBinary = join(repositoryRoot, 'target', 'release', 'craxii-server');
const controlArgument = '--test-failpoint-control-v1';
const controlProtocol = 'CRAXII_TEST_CONTROL_V1';
const requiredFailpoints = [
  'after_message_transaction_commit',
  'after_work_claim_commit',
  'after_context_manifest_commit',
  'after_model_intent_commit',
  'after_first_provider_delta',
  'after_model_response_commit',
  'after_tool_requested_commit',
  'after_tool_dispatch_intent_commit',
  'after_tool_process_spawn',
  'after_tool_process_exit_before_outcome_commit',
  'after_artifact_rename_before_db_commit',
  'after_assistant_message_commit',
  'after_cancel_requested_commit',
  'during_graceful_shutdown',
];

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

try {
  const execution = spawnSync(releaseBinary, [controlArgument], {
    cwd: repositoryRoot,
    encoding: 'utf8',
    input: `${controlProtocol}\n`,
    timeout: 5_000,
    maxBuffer: 16 * 1024,
  });
  if (execution.error) {
    throw new Error(`standard release binary could not be inspected: ${execution.error.message}`);
  }
  assert(execution.signal === null, 'standard release binary did not exit normally');
  assert(execution.status === 1, `standard release hidden-control probe exited ${execution.status}`);
  assert(execution.stdout === '', 'standard release hidden-control probe emitted stdout');
  assert(
    execution.stderr === 'craxii fatal: invalid_cli\n',
    'standard release binary recognized or mishandled the hidden test-control surface',
  );

  const binary = readFileSync(releaseBinary);
  assert(
    !binary.includes(Buffer.from(controlProtocol)),
    'standard release binary contains test-control protocol magic',
  );
  for (const name of requiredFailpoints) {
    assert(
      !binary.includes(Buffer.from(name)),
      `standard release binary contains reserved failpoint name ${name}`,
    );
  }

  console.log(
    'Release failpoint inspection passed: hidden control unrecognized, protocol magic absent, 14 reserved names absent',
  );
} catch (error) {
  console.error(`Release failpoint inspection failed: ${error.message}`);
  process.exitCode = 1;
}
