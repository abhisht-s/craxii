#!/usr/bin/env node

import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDirectory, '..');

function read(path) {
  return readFileSync(join(repositoryRoot, path), 'utf8');
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

const spanLocations = new Map([
  ['service_startup', 'backend/src/adapters/telemetry.rs'],
  ['database_migration', 'backend/src/adapters/sqlite/runtime.rs'],
  ['startup_recovery', 'backend/src/application/runtime.rs'],
  ['http_request', 'backend/src/adapters/http.rs'],
  ['client_command', 'backend/src/adapters/http.rs'],
  ['websocket_connection', 'backend/src/adapters/http.rs'],
  ['event_replay', 'backend/src/adapters/http.rs'],
  ['work_queue_wait', 'backend/src/application/scheduler.rs'],
  ['work_execution', 'backend/src/application/scheduler.rs'],
  ['context_assembly', 'backend/src/application/context_assembler.rs'],
  ['model_selection', 'backend/src/application/model_selection.rs'],
  ['model_invocation_attempt', 'backend/src/application/model_gateway.rs'],
  ['provider_stream', 'backend/src/application/model_gateway.rs'],
  ['tool_execution_service', 'backend/src/application/tool_execution_service.rs'],
  ['workstation_read_file', 'backend/src/adapters/local_workstation.rs'],
  ['workstation_execute', 'backend/src/adapters/local_workstation.rs'],
  ['process_cleanup', 'backend/src/adapters/local_workstation/execution.rs'],
  ['artifact_write', 'backend/src/adapters/artifacts/local.rs'],
  ['journal_transaction', 'backend/src/adapters/sqlite/transaction.rs'],
  ['sqlite_checkpoint', 'backend/src/adapters/sqlite/runtime.rs'],
]);

for (const [span, path] of spanLocations) {
  assert(read(path).includes(`"${span}"`), `required Stage 23 span is absent: ${span}`);
}

const admin = read('backend/src/bin/craxii-admin.rs');
for (const command of [
  'preflight',
  'verify-state',
  'inspect-work',
  'inspect-runtime',
  'evidence-export',
]) {
  assert(admin.includes(`OsStr::new("${command}")`), `offline command is absent: ${command}`);
}

const http = read('backend/src/adapters/http.rs');
for (const command of ['preflight', 'verify-state', 'inspect-work', 'inspect-runtime', 'evidence-export']) {
  assert(!http.includes(`"/${command}`), `offline command leaked into the public HTTP adapter: ${command}`);
}

const submitMessage = http.slice(
  http.indexOf('async fn submit_message('),
  http.indexOf('async fn cancel_work('),
);
for (const field of [
  'event_name = "client_command_terminal"',
  'request_id = %context.request_id',
  'command_kind = "message"',
  'conversation_id = %conversation_id',
  'client_message_id = %client_message_id',
  'message_id = %receipt.message_id',
  'work_id = %receipt.work_id',
  'result_class = if duplicate',
  'duration_micros',
]) {
  assert(submitMessage.includes(field), `message command observation is missing: ${field}`);
}
assert(
  submitMessage.includes('command_span.in_scope(||'),
  'message command observation is not emitted inside client_command',
);

const cancelWork = http.slice(
  http.indexOf('async fn cancel_work('),
  http.indexOf('#[derive(Deserialize)]', http.indexOf('async fn cancel_work(')),
);
for (const field of [
  'event_name = "client_command_terminal"',
  'request_id = %context.request_id',
  'command_kind = "cancellation"',
  'cancellation_command_id = %cancellation_command_id',
  'work_id = %receipt.work_id',
  'resulting_work_state = receipt.resulting_work_state.as_str()',
  'journal_cursor = receipt.committed_cursor.get()',
  'cleanup_pending = receipt.cleanup.is_pending()',
]) {
  assert(cancelWork.includes(field), `cancellation command observation is missing: ${field}`);
}
assert(
  cancelWork.includes('command_span.in_scope(||'),
  'cancellation observation is not emitted inside client_command',
);

const modelGateway = read('backend/src/application/model_gateway.rs');
assert(
  modelGateway.includes('attempt_span.in_scope(|| {\n                observe_model_attempt('),
  'model terminal observation is not emitted inside model_invocation_attempt',
);
const modelObservation = modelGateway.slice(
  modelGateway.indexOf('fn observe_model_attempt('),
  modelGateway.indexOf('enum StreamTerminal'),
);
for (const field of [
  'event_name = "model_attempt_terminal"',
  'work_id = observation.work_id.as_str()',
  'logical_invocation_id = observation.logical_invocation_id.as_str()',
  'model_invocation_id = observation.model_invocation_id.as_str()',
  'attempt_ordinal = observation.attempt_ordinal',
  'target = observation.target.as_str()',
  'provider = observation.provider.as_str()',
  'model = observation.model.as_str()',
  'request_sha256 = observation.request_sha256.as_str()',
  'request_bytes = observation.request_bytes',
  'total_latency_ms',
  'provider_request_digest',
  'event_name = "model_attempt_retry_scheduled"',
  'retry_reason',
  'retry_delay_ms',
]) {
  assert(modelObservation.includes(field), `model attempt observation is missing: ${field}`);
}

const telemetry = read('backend/src/adapters/telemetry.rs');
assert(
  !telemetry.includes('.with_span_events('),
  'generic span lifecycle output must not replace explicit Stage 23 observations',
);
const productionCorrelationTest = read('backend/tests/stage23.rs');
for (const evidence of [
  'production_json_reconstructs_request_command_work_and_model_attempt',
  'production_test_dispatch',
  'http_request_terminal',
  'client_command_terminal',
  'work_terminal',
  'model_attempt_terminal',
]) {
  assert(
    productionCorrelationTest.includes(evidence),
    `production JSON correlation proof is missing: ${evidence}`,
  );
}
assert(
  read('scripts/verify-stage23-observability').includes('--test stage23'),
  'Stage 23 verifier does not run the production JSON correlation proof',
);

const evidence = read('backend/src/application/evidence_inspection.rs');
assert(
  evidence.includes('craxii.operator-evidence/v1'),
  'operator evidence format is not explicitly versioned',
);
assert(evidence.includes('read_only_noncanonical'), 'operator evidence role is not noncanonical');
assert(
  read('backend/src/adapters/sqlite/runtime.rs').includes('start_read_only'),
  'SQLite offline read-only guard is absent',
);
assert(
  read('backend/src/adapters/artifacts/local.rs').includes('open_read_only'),
  'artifact read-only inspection mode is absent',
);

const diagnostics = read('clients/macos/CraxiiClient/Sources/CraxiiClientCore/ClientDiagnostics.swift');
for (const symbol of [
  'ClientDiagnosticEvent',
  'ClientDiagnosticRecording',
  'NoopClientDiagnosticRecorder',
  'InMemoryClientDiagnosticRecorder',
]) {
  assert(diagnostics.includes(symbol), `typed client diagnostic component is absent: ${symbol}`);
}
const osLog = read(
  'clients/macos/CraxiiClient/Sources/CraxiiAppleAdapters/OSLogClientDiagnosticRecorder.swift',
);
assert(osLog.includes('import OSLog'), 'native diagnostics do not use Apple OSLog');
assert(osLog.includes('Logger'), 'native diagnostics do not use os.Logger');

assert(
  read('backend/src/adapters/sqlite/schema.rs').includes(
    'pub const MAX_SUPPORTED_SCHEMA_VERSION: i64 = 4;',
  ),
  'Stage 23 must not change the durable schema version',
);
assert(
  read('backend/src/protocol.rs').includes('pub const PROTOCOL_VERSION: u64 = 1;'),
  'Stage 23 must not change the public protocol version',
);

const telemetryTests = read('backend/src/adapters/telemetry.rs');
for (const sentinel of [
  'SENTINEL_AUTH_23',
  'SENTINEL_PROVIDER_API_KEY_23',
  'SENTINEL_REQUEST_BODY_23',
  'SENTINEL_USER_MESSAGE_23',
  'SENTINEL_MODEL_PROMPT_23',
  'SENTINEL_MODEL_OUTPUT_23',
  'SENTINEL_MODEL_REFUSAL_23',
  'SENTINEL_TOOL_ARGUMENTS_23',
  'SENTINEL_SHELL_COMMAND_23',
  'SENTINEL_STDOUT_23',
  'SENTINEL_STDERR_23',
  'SENTINEL_FILE_CONTENT_23',
  'SENTINEL_ENV_SECRET_23',
  'SENTINEL_URL_SECRET_23',
  'SENTINEL_PROVIDER_ERROR_BODY_23',
  'SENTINEL_KEYCHAIN_TOKEN_23',
]) {
  assert(telemetryTests.includes(sentinel), `trace sentinel is absent: ${sentinel}`);
}

console.log(
  'Stage 23 structural inspection passed: spans, offline evidence, redaction, diagnostics, protocol, and schema boundaries are present',
);
