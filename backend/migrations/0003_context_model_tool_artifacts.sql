CREATE TABLE artifacts (
    artifact_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(artifact_id) = 36
            AND substr(artifact_id, 9, 1) = '-'
            AND substr(artifact_id, 14, 1) = '-'
            AND substr(artifact_id, 19, 1) = '-'
            AND substr(artifact_id, 24, 1) = '-'
            AND substr(artifact_id, 15, 1) = '7'
            AND substr(artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(artifact_id, '-', '')) = 32
            AND replace(artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    craxii_id TEXT NOT NULL
        CHECK (
            length(craxii_id) = 36
            AND substr(craxii_id, 9, 1) = '-'
            AND substr(craxii_id, 14, 1) = '-'
            AND substr(craxii_id, 19, 1) = '-'
            AND substr(craxii_id, 24, 1) = '-'
            AND substr(craxii_id, 15, 1) = '7'
            AND substr(craxii_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(craxii_id, '-', '')) = 32
            AND replace(craxii_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    producing_work_id TEXT NULL
        CHECK (
            producing_work_id IS NULL
            OR (
                length(producing_work_id) = 36
                AND substr(producing_work_id, 9, 1) = '-'
                AND substr(producing_work_id, 14, 1) = '-'
                AND substr(producing_work_id, 19, 1) = '-'
                AND substr(producing_work_id, 24, 1) = '-'
                AND substr(producing_work_id, 15, 1) = '7'
                AND substr(producing_work_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(producing_work_id, '-', '')) = 32
                AND replace(producing_work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    producer_kind TEXT NULL
        CHECK (producer_kind IS NULL OR producer_kind IN ('model_invocation', 'tool_execution')),
    producer_id TEXT NULL
        CHECK (
            producer_id IS NULL
            OR (
                length(producer_id) = 36
                AND substr(producer_id, 9, 1) = '-'
                AND substr(producer_id, 14, 1) = '-'
                AND substr(producer_id, 19, 1) = '-'
                AND substr(producer_id, 24, 1) = '-'
                AND substr(producer_id, 15, 1) = '7'
                AND substr(producer_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(producer_id, '-', '')) = 32
                AND replace(producer_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    backend TEXT NOT NULL CHECK (backend = 'local'),
    storage_key TEXT NOT NULL
        CHECK (length(storage_key) = 74),
    sha256 TEXT NOT NULL
        CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    captured_byte_count INTEGER NOT NULL CHECK (captured_byte_count >= 0),
    observed_byte_count INTEGER NULL CHECK (observed_byte_count IS NULL OR observed_byte_count >= 0),
    mime_type TEXT NOT NULL CHECK (length(mime_type) BETWEEN 1 AND 255 AND trim(mime_type) = mime_type),
    encoding TEXT NULL CHECK (encoding IS NULL OR (length(encoding) BETWEEN 1 AND 64 AND trim(encoding) = encoding)),
    logical_name TEXT NULL CHECK (logical_name IS NULL OR (length(logical_name) BETWEEN 1 AND 255 AND trim(logical_name) = logical_name)),
    retention_class TEXT NOT NULL
        CHECK (retention_class IN ('canonical_evidence', 'diagnostic', 'regenerable')),
    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    compression TEXT NULL CHECK (compression IS NULL OR (length(compression) BETWEEN 1 AND 64 AND trim(compression) = compression)),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    CHECK (storage_key = 'sha256/' || substr(sha256, 1, 2) || '/' || sha256),
    CHECK (
        (producer_kind IS NULL AND producer_id IS NULL)
        OR (producer_kind IS NOT NULL AND producer_id IS NOT NULL AND producing_work_id IS NOT NULL)
    ),
    CHECK (
        (observed_byte_count IS NULL AND truncated = 0)
        OR (
            observed_byte_count IS NOT NULL
            AND observed_byte_count >= captured_byte_count
            AND truncated = (observed_byte_count > captured_byte_count)
        )
    ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (producing_work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_artifacts_storage_key
    ON artifacts (storage_key);
CREATE INDEX ix_artifacts_content
    ON artifacts (sha256, captured_byte_count, backend);
CREATE INDEX ix_artifacts_producing_work
    ON artifacts (producing_work_id, created_at);
CREATE INDEX ix_artifacts_producer_kind_id
    ON artifacts (producer_kind, producer_id)
    WHERE producer_kind IS NOT NULL;

CREATE TABLE context_manifests (
    context_manifest_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(context_manifest_id) = 36
            AND substr(context_manifest_id, 9, 1) = '-'
            AND substr(context_manifest_id, 14, 1) = '-'
            AND substr(context_manifest_id, 19, 1) = '-'
            AND substr(context_manifest_id, 24, 1) = '-'
            AND substr(context_manifest_id, 15, 1) = '7'
            AND substr(context_manifest_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(context_manifest_id, '-', '')) = 32
            AND replace(context_manifest_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    work_id TEXT NOT NULL
        CHECK (
            length(work_id) = 36
            AND substr(work_id, 9, 1) = '-'
            AND substr(work_id, 14, 1) = '-'
            AND substr(work_id, 19, 1) = '-'
            AND substr(work_id, 24, 1) = '-'
            AND substr(work_id, 15, 1) = '7'
            AND substr(work_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(work_id, '-', '')) = 32
            AND replace(work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    logical_invocation_id TEXT NOT NULL
        CHECK (
            length(logical_invocation_id) = 36
            AND substr(logical_invocation_id, 9, 1) = '-'
            AND substr(logical_invocation_id, 14, 1) = '-'
            AND substr(logical_invocation_id, 19, 1) = '-'
            AND substr(logical_invocation_id, 24, 1) = '-'
            AND substr(logical_invocation_id, 15, 1) = '7'
            AND substr(logical_invocation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(logical_invocation_id, '-', '')) = 32
            AND replace(logical_invocation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    model_target_id TEXT NOT NULL
        CHECK (length(model_target_id) BETWEEN 1 AND 64 AND model_target_id GLOB '[a-z0-9]*' AND model_target_id NOT GLOB '*[^a-z0-9._-]*'),
    provider_id TEXT NOT NULL
        CHECK (length(provider_id) BETWEEN 1 AND 64 AND provider_id GLOB '[a-z0-9]*' AND provider_id NOT GLOB '*[^a-z0-9._-]*'),
    provider_model_id TEXT NOT NULL
        CHECK (length(provider_model_id) BETWEEN 1 AND 128 AND trim(provider_model_id) = provider_model_id),
    target_configuration_version INTEGER NOT NULL CHECK (target_configuration_version > 0),
    model_capabilities_json TEXT NOT NULL
        CHECK (length(model_capabilities_json) <= 16384 AND json_valid(model_capabilities_json) AND json_type(model_capabilities_json) = 'object'),
    assembler_version TEXT NOT NULL
        CHECK (length(assembler_version) BETWEEN 1 AND 64 AND trim(assembler_version) = assembler_version),
    context_policy_version TEXT NOT NULL
        CHECK (length(context_policy_version) BETWEEN 1 AND 64 AND trim(context_policy_version) = context_policy_version),
    system_prompt_fingerprint TEXT NOT NULL
        CHECK (length(system_prompt_fingerprint) = 64 AND system_prompt_fingerprint NOT GLOB '*[^0-9a-f]*'),
    toolset_fingerprint TEXT NOT NULL
        CHECK (length(toolset_fingerprint) = 64 AND toolset_fingerprint NOT GLOB '*[^0-9a-f]*'),
    eligibility_cutoff_json TEXT NOT NULL
        CHECK (length(eligibility_cutoff_json) <= 65536 AND json_valid(eligibility_cutoff_json) AND json_type(eligibility_cutoff_json) = 'object'),
    source_count INTEGER NOT NULL CHECK (source_count >= 0),
    canonical_byte_count INTEGER NOT NULL CHECK (canonical_byte_count >= 0),
    rendered_request_byte_count INTEGER NOT NULL CHECK (rendered_request_byte_count >= 0),
    estimated_input_tokens INTEGER NOT NULL CHECK (estimated_input_tokens BETWEEN 0 AND 2147483647),
    token_estimator_id TEXT NOT NULL
        CHECK (length(token_estimator_id) BETWEEN 1 AND 64 AND trim(token_estimator_id) = token_estimator_id),
    context_window_tokens INTEGER NOT NULL CHECK (context_window_tokens BETWEEN 1 AND 2147483647),
    reserved_output_tokens INTEGER NOT NULL CHECK (reserved_output_tokens BETWEEN 0 AND 2147483647),
    utilization_basis_points INTEGER NOT NULL CHECK (utilization_basis_points BETWEEN 0 AND 10000),
    manifest_sha256 TEXT NOT NULL
        CHECK (length(manifest_sha256) = 64 AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'),
    rendered_request_sha256 TEXT NOT NULL
        CHECK (length(rendered_request_sha256) = 64 AND rendered_request_sha256 NOT GLOB '*[^0-9a-f]*'),
    rendered_request_artifact_id TEXT NULL
        CHECK (
            rendered_request_artifact_id IS NULL
            OR (
                length(rendered_request_artifact_id) = 36
                AND substr(rendered_request_artifact_id, 9, 1) = '-'
                AND substr(rendered_request_artifact_id, 14, 1) = '-'
                AND substr(rendered_request_artifact_id, 19, 1) = '-'
                AND substr(rendered_request_artifact_id, 24, 1) = '-'
                AND substr(rendered_request_artifact_id, 15, 1) = '7'
                AND substr(rendered_request_artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(rendered_request_artifact_id, '-', '')) = 32
                AND replace(rendered_request_artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    omissions_json TEXT NOT NULL
        CHECK (length(omissions_json) <= 65536 AND json_valid(omissions_json) AND json_type(omissions_json) = 'object'),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    CHECK (reserved_output_tokens <= context_window_tokens),
    CHECK (estimated_input_tokens + reserved_output_tokens <= context_window_tokens),
    CHECK (
        utilization_basis_points = (
            ((estimated_input_tokens + reserved_output_tokens) * 10000 + context_window_tokens - 1)
            / context_window_tokens
        )
    ),
    FOREIGN KEY (work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (rendered_request_artifact_id) REFERENCES artifacts (artifact_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_context_manifests_logical_invocation
    ON context_manifests (logical_invocation_id);
CREATE INDEX ix_context_manifests_work_created
    ON context_manifests (work_id, created_at);

CREATE TABLE context_manifest_sources (
    context_manifest_id TEXT NOT NULL
        CHECK (
            length(context_manifest_id) = 36
            AND substr(context_manifest_id, 9, 1) = '-'
            AND substr(context_manifest_id, 14, 1) = '-'
            AND substr(context_manifest_id, 19, 1) = '-'
            AND substr(context_manifest_id, 24, 1) = '-'
            AND substr(context_manifest_id, 15, 1) = '7'
            AND substr(context_manifest_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(context_manifest_id, '-', '')) = 32
            AND replace(context_manifest_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    position INTEGER NOT NULL CHECK (position > 0),
    source_kind TEXT NOT NULL
        CHECK (
            source_kind IN (
                'system_instruction',
                'developer_instruction',
                'workstation_capability_summary',
                'workspace_identity',
                'tool_definition',
                'user_message',
                'active_trigger',
                'assistant_message',
                'completed_model_output',
                'observed_tool_result',
                'artifact_content',
                'synthetic_failure',
                'synthetic_interruption',
                'synthetic_outcome_unknown',
                'synthetic_draft_status',
                'provider_native_continuation'
            )
        ),
    event_id TEXT NULL
        CHECK (
            event_id IS NULL
            OR (
                length(event_id) = 36
                AND substr(event_id, 9, 1) = '-'
                AND substr(event_id, 14, 1) = '-'
                AND substr(event_id, 19, 1) = '-'
                AND substr(event_id, 24, 1) = '-'
                AND substr(event_id, 15, 1) = '7'
                AND substr(event_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(event_id, '-', '')) = 32
                AND replace(event_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    artifact_id TEXT NULL
        CHECK (
            artifact_id IS NULL
            OR (
                length(artifact_id) = 36
                AND substr(artifact_id, 9, 1) = '-'
                AND substr(artifact_id, 14, 1) = '-'
                AND substr(artifact_id, 19, 1) = '-'
                AND substr(artifact_id, 24, 1) = '-'
                AND substr(artifact_id, 15, 1) = '7'
                AND substr(artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(artifact_id, '-', '')) = 32
                AND replace(artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    source_record_kind TEXT NULL
        CHECK (
            source_record_kind IS NULL
            OR source_record_kind IN (
                'instruction_version',
                'workstation',
                'workspace',
                'tool_definition',
                'message',
                'model_invocation',
                'tool_execution',
                'work'
            )
        ),
    source_record_id TEXT NULL
        CHECK (source_record_id IS NULL OR (length(source_record_id) BETWEEN 1 AND 255 AND trim(source_record_id) = source_record_id)),
    model_role TEXT NULL
        CHECK (model_role IS NULL OR model_role IN ('system', 'developer', 'user', 'assistant', 'tool')),
    item_class TEXT NULL
        CHECK (item_class IS NULL OR (length(item_class) BETWEEN 1 AND 64 AND item_class GLOB '[a-z0-9]*' AND item_class NOT GLOB '*[^a-z0-9._-]*')),
    source_content_sha256 TEXT NOT NULL
        CHECK (length(source_content_sha256) = 64 AND source_content_sha256 NOT GLOB '*[^0-9a-f]*'),
    rendered_byte_contribution INTEGER NOT NULL CHECK (rendered_byte_contribution >= 0),
    transform_json TEXT NOT NULL
        CHECK (length(transform_json) <= 65536 AND json_valid(transform_json) AND json_type(transform_json) = 'object'),
    PRIMARY KEY (context_manifest_id, position),
    CHECK ((source_record_kind IS NULL) = (source_record_id IS NULL)),
    CHECK (
        (event_id IS NOT NULL)
        + (artifact_id IS NOT NULL)
        + (source_record_kind IS NOT NULL) = 1
    ),
    FOREIGN KEY (context_manifest_id) REFERENCES context_manifests (context_manifest_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (event_id) REFERENCES journal_events (event_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (artifact_id) REFERENCES artifacts (artifact_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_context_manifest_sources_event
    ON context_manifest_sources (event_id)
    WHERE event_id IS NOT NULL;
CREATE INDEX ix_context_manifest_sources_artifact
    ON context_manifest_sources (artifact_id)
    WHERE artifact_id IS NOT NULL;

CREATE TABLE model_invocations (
    model_invocation_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(model_invocation_id) = 36
            AND substr(model_invocation_id, 9, 1) = '-'
            AND substr(model_invocation_id, 14, 1) = '-'
            AND substr(model_invocation_id, 19, 1) = '-'
            AND substr(model_invocation_id, 24, 1) = '-'
            AND substr(model_invocation_id, 15, 1) = '7'
            AND substr(model_invocation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(model_invocation_id, '-', '')) = 32
            AND replace(model_invocation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    logical_invocation_id TEXT NOT NULL
        CHECK (
            length(logical_invocation_id) = 36
            AND substr(logical_invocation_id, 9, 1) = '-'
            AND substr(logical_invocation_id, 14, 1) = '-'
            AND substr(logical_invocation_id, 19, 1) = '-'
            AND substr(logical_invocation_id, 24, 1) = '-'
            AND substr(logical_invocation_id, 15, 1) = '7'
            AND substr(logical_invocation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(logical_invocation_id, '-', '')) = 32
            AND replace(logical_invocation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    work_id TEXT NOT NULL
        CHECK (
            length(work_id) = 36
            AND substr(work_id, 9, 1) = '-'
            AND substr(work_id, 14, 1) = '-'
            AND substr(work_id, 19, 1) = '-'
            AND substr(work_id, 24, 1) = '-'
            AND substr(work_id, 15, 1) = '7'
            AND substr(work_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(work_id, '-', '')) = 32
            AND replace(work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    runtime_instance_id TEXT NOT NULL
        CHECK (
            length(runtime_instance_id) = 36
            AND substr(runtime_instance_id, 9, 1) = '-'
            AND substr(runtime_instance_id, 14, 1) = '-'
            AND substr(runtime_instance_id, 19, 1) = '-'
            AND substr(runtime_instance_id, 24, 1) = '-'
            AND substr(runtime_instance_id, 15, 1) = '7'
            AND substr(runtime_instance_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(runtime_instance_id, '-', '')) = 32
            AND replace(runtime_instance_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    context_manifest_id TEXT NOT NULL
        CHECK (
            length(context_manifest_id) = 36
            AND substr(context_manifest_id, 9, 1) = '-'
            AND substr(context_manifest_id, 14, 1) = '-'
            AND substr(context_manifest_id, 19, 1) = '-'
            AND substr(context_manifest_id, 24, 1) = '-'
            AND substr(context_manifest_id, 15, 1) = '7'
            AND substr(context_manifest_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(context_manifest_id, '-', '')) = 32
            AND replace(context_manifest_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    agent_step_no INTEGER NOT NULL CHECK (agent_step_no > 0),
    attempt_no INTEGER NOT NULL CHECK (attempt_no > 0),
    retry_of_invocation_id TEXT NULL
        CHECK (
            retry_of_invocation_id IS NULL
            OR (
                length(retry_of_invocation_id) = 36
                AND substr(retry_of_invocation_id, 9, 1) = '-'
                AND substr(retry_of_invocation_id, 14, 1) = '-'
                AND substr(retry_of_invocation_id, 19, 1) = '-'
                AND substr(retry_of_invocation_id, 24, 1) = '-'
                AND substr(retry_of_invocation_id, 15, 1) = '7'
                AND substr(retry_of_invocation_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(retry_of_invocation_id, '-', '')) = 32
                AND replace(retry_of_invocation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    model_target_id TEXT NOT NULL
        CHECK (length(model_target_id) BETWEEN 1 AND 64 AND model_target_id GLOB '[a-z0-9]*' AND model_target_id NOT GLOB '*[^a-z0-9._-]*'),
    provider_id TEXT NOT NULL
        CHECK (length(provider_id) BETWEEN 1 AND 64 AND provider_id GLOB '[a-z0-9]*' AND provider_id NOT GLOB '*[^a-z0-9._-]*'),
    provider_model_id TEXT NOT NULL
        CHECK (length(provider_model_id) BETWEEN 1 AND 128 AND trim(provider_model_id) = provider_model_id),
    target_configuration_version INTEGER NOT NULL CHECK (target_configuration_version > 0),
    model_capabilities_json TEXT NOT NULL
        CHECK (length(model_capabilities_json) <= 16384 AND json_valid(model_capabilities_json) AND json_type(model_capabilities_json) = 'object'),
    selection_reason TEXT NOT NULL CHECK (selection_reason IN ('explicit', 'configured_default')),
    required_capabilities_json TEXT NOT NULL
        CHECK (length(required_capabilities_json) <= 16384 AND json_valid(required_capabilities_json) AND json_type(required_capabilities_json) = 'object'),
    provider_options_json TEXT NOT NULL
        CHECK (length(provider_options_json) <= 65536 AND json_valid(provider_options_json) AND json_type(provider_options_json) = 'object'),
    state TEXT NOT NULL
        CHECK (state IN ('requesting', 'streaming', 'completed', 'failed', 'cancelled_locally', 'provider_outcome_unknown')),
    request_sha256 TEXT NOT NULL
        CHECK (length(request_sha256) = 64 AND request_sha256 NOT GLOB '*[^0-9a-f]*'),
    request_artifact_id TEXT NULL
        CHECK (
            request_artifact_id IS NULL
            OR (
                length(request_artifact_id) = 36
                AND substr(request_artifact_id, 9, 1) = '-'
                AND substr(request_artifact_id, 14, 1) = '-'
                AND substr(request_artifact_id, 19, 1) = '-'
                AND substr(request_artifact_id, 24, 1) = '-'
                AND substr(request_artifact_id, 15, 1) = '7'
                AND substr(request_artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(request_artifact_id, '-', '')) = 32
                AND replace(request_artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    response_sha256 TEXT NULL
        CHECK (response_sha256 IS NULL OR (length(response_sha256) = 64 AND response_sha256 NOT GLOB '*[^0-9a-f]*')),
    response_artifact_id TEXT NULL
        CHECK (
            response_artifact_id IS NULL
            OR (
                length(response_artifact_id) = 36
                AND substr(response_artifact_id, 9, 1) = '-'
                AND substr(response_artifact_id, 14, 1) = '-'
                AND substr(response_artifact_id, 19, 1) = '-'
                AND substr(response_artifact_id, 24, 1) = '-'
                AND substr(response_artifact_id, 15, 1) = '7'
                AND substr(response_artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(response_artifact_id, '-', '')) = 32
                AND replace(response_artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    normalized_output_json TEXT NULL
        CHECK (normalized_output_json IS NULL OR (length(normalized_output_json) <= 262144 AND json_valid(normalized_output_json) AND json_type(normalized_output_json) = 'object')),
    provider_request_id TEXT NULL
        CHECK (provider_request_id IS NULL OR (length(provider_request_id) BETWEEN 1 AND 255 AND trim(provider_request_id) = provider_request_id)),
    provider_response_id TEXT NULL
        CHECK (provider_response_id IS NULL OR (length(provider_response_id) BETWEEN 1 AND 255 AND trim(provider_response_id) = provider_response_id)),
    started_at TEXT NOT NULL
        CHECK (
            length(started_at) = 27
            AND started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    first_byte_at TEXT NULL
        CHECK (
            first_byte_at IS NULL
            OR (
                length(first_byte_at) = 27
                AND first_byte_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    first_output_at TEXT NULL
        CHECK (
            first_output_at IS NULL
            OR (
                length(first_output_at) = 27
                AND first_output_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    completed_at TEXT NULL
        CHECK (
            completed_at IS NULL
            OR (
                length(completed_at) = 27
                AND completed_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    input_tokens INTEGER NULL CHECK (input_tokens IS NULL OR input_tokens >= 0),
    cached_input_tokens INTEGER NULL CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
    output_tokens INTEGER NULL CHECK (output_tokens IS NULL OR output_tokens >= 0),
    reasoning_tokens INTEGER NULL CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
    total_tokens INTEGER NULL CHECK (total_tokens IS NULL OR total_tokens >= 0),
    stop_reason TEXT NULL CHECK (stop_reason IS NULL OR (length(stop_reason) BETWEEN 1 AND 64 AND trim(stop_reason) = stop_reason)),
    tool_call_count INTEGER NULL CHECK (tool_call_count IS NULL OR tool_call_count >= 0),
    draft_exposed INTEGER NOT NULL DEFAULT 0 CHECK (draft_exposed IN (0, 1)),
    normalized_error_json TEXT NULL
        CHECK (normalized_error_json IS NULL OR (length(normalized_error_json) <= 65536 AND json_valid(normalized_error_json) AND json_type(normalized_error_json) = 'object')),
    CHECK ((attempt_no = 1 AND retry_of_invocation_id IS NULL) OR (attempt_no > 1 AND retry_of_invocation_id IS NOT NULL)),
    CHECK (retry_of_invocation_id IS NULL OR retry_of_invocation_id <> model_invocation_id),
    CHECK (first_byte_at IS NULL OR first_byte_at >= started_at),
    CHECK (first_output_at IS NULL OR (first_byte_at IS NOT NULL AND first_output_at >= first_byte_at)),
    CHECK (completed_at IS NULL OR completed_at >= started_at),
    CHECK (cached_input_tokens IS NULL OR input_tokens IS NULL OR cached_input_tokens <= input_tokens),
    CHECK (response_artifact_id IS NULL OR response_sha256 IS NOT NULL),
    CHECK (
        (
            state = 'requesting'
            AND first_byte_at IS NULL
            AND first_output_at IS NULL
            AND completed_at IS NULL
            AND response_sha256 IS NULL
            AND response_artifact_id IS NULL
            AND normalized_output_json IS NULL
            AND provider_response_id IS NULL
            AND input_tokens IS NULL
            AND cached_input_tokens IS NULL
            AND output_tokens IS NULL
            AND reasoning_tokens IS NULL
            AND total_tokens IS NULL
            AND stop_reason IS NULL
            AND tool_call_count IS NULL
            AND draft_exposed = 0
            AND normalized_error_json IS NULL
        )
        OR (
            state = 'streaming'
            AND first_byte_at IS NOT NULL
            AND completed_at IS NULL
            AND response_sha256 IS NULL
            AND response_artifact_id IS NULL
            AND normalized_output_json IS NULL
            AND input_tokens IS NULL
            AND cached_input_tokens IS NULL
            AND output_tokens IS NULL
            AND reasoning_tokens IS NULL
            AND total_tokens IS NULL
            AND stop_reason IS NULL
            AND tool_call_count IS NULL
            AND normalized_error_json IS NULL
        )
        OR (
            state = 'completed'
            AND first_byte_at IS NOT NULL
            AND first_output_at IS NOT NULL
            AND completed_at IS NOT NULL
            AND response_sha256 IS NOT NULL
            AND normalized_output_json IS NOT NULL
            AND stop_reason IS NOT NULL
            AND tool_call_count IS NOT NULL
            AND normalized_error_json IS NULL
        )
        OR (
            state IN ('failed', 'cancelled_locally', 'provider_outcome_unknown')
            AND completed_at IS NOT NULL
            AND response_sha256 IS NULL
            AND response_artifact_id IS NULL
            AND normalized_output_json IS NULL
            AND input_tokens IS NULL
            AND cached_input_tokens IS NULL
            AND output_tokens IS NULL
            AND reasoning_tokens IS NULL
            AND total_tokens IS NULL
            AND stop_reason IS NULL
            AND tool_call_count IS NULL
            AND normalized_error_json IS NOT NULL
        )
    ),
    FOREIGN KEY (work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (runtime_instance_id) REFERENCES runtime_instances (runtime_instance_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (context_manifest_id) REFERENCES context_manifests (context_manifest_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (retry_of_invocation_id) REFERENCES model_invocations (model_invocation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (request_artifact_id) REFERENCES artifacts (artifact_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (response_artifact_id) REFERENCES artifacts (artifact_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_model_invocations_logical_attempt
    ON model_invocations (logical_invocation_id, attempt_no);
CREATE UNIQUE INDEX ux_model_invocations_work_step_attempt
    ON model_invocations (work_id, agent_step_no, attempt_no);
CREATE UNIQUE INDEX ux_model_invocations_retry_of
    ON model_invocations (retry_of_invocation_id)
    WHERE retry_of_invocation_id IS NOT NULL;
CREATE UNIQUE INDEX ux_model_invocations_one_nonterminal_per_work
    ON model_invocations (work_id)
    WHERE state IN ('requesting', 'streaming');
CREATE INDEX ix_model_invocations_runtime_nonterminal
    ON model_invocations (runtime_instance_id, state, work_id)
    WHERE state IN ('requesting', 'streaming');
CREATE INDEX ix_model_invocations_context_attempt
    ON model_invocations (context_manifest_id, attempt_no);

CREATE TABLE tool_executions (
    tool_execution_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(tool_execution_id) = 36
            AND substr(tool_execution_id, 9, 1) = '-'
            AND substr(tool_execution_id, 14, 1) = '-'
            AND substr(tool_execution_id, 19, 1) = '-'
            AND substr(tool_execution_id, 24, 1) = '-'
            AND substr(tool_execution_id, 15, 1) = '7'
            AND substr(tool_execution_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(tool_execution_id, '-', '')) = 32
            AND replace(tool_execution_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    execution_id TEXT NOT NULL
        CHECK (
            length(execution_id) = 36
            AND substr(execution_id, 9, 1) = '-'
            AND substr(execution_id, 14, 1) = '-'
            AND substr(execution_id, 19, 1) = '-'
            AND substr(execution_id, 24, 1) = '-'
            AND substr(execution_id, 15, 1) = '7'
            AND substr(execution_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(execution_id, '-', '')) = 32
            AND replace(execution_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    work_id TEXT NOT NULL
        CHECK (
            length(work_id) = 36
            AND substr(work_id, 9, 1) = '-'
            AND substr(work_id, 14, 1) = '-'
            AND substr(work_id, 19, 1) = '-'
            AND substr(work_id, 24, 1) = '-'
            AND substr(work_id, 15, 1) = '7'
            AND substr(work_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(work_id, '-', '')) = 32
            AND replace(work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    source_model_invocation_id TEXT NOT NULL
        CHECK (
            length(source_model_invocation_id) = 36
            AND substr(source_model_invocation_id, 9, 1) = '-'
            AND substr(source_model_invocation_id, 14, 1) = '-'
            AND substr(source_model_invocation_id, 19, 1) = '-'
            AND substr(source_model_invocation_id, 24, 1) = '-'
            AND substr(source_model_invocation_id, 15, 1) = '7'
            AND substr(source_model_invocation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(source_model_invocation_id, '-', '')) = 32
            AND replace(source_model_invocation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    runtime_instance_id TEXT NOT NULL
        CHECK (
            length(runtime_instance_id) = 36
            AND substr(runtime_instance_id, 9, 1) = '-'
            AND substr(runtime_instance_id, 14, 1) = '-'
            AND substr(runtime_instance_id, 19, 1) = '-'
            AND substr(runtime_instance_id, 24, 1) = '-'
            AND substr(runtime_instance_id, 15, 1) = '7'
            AND substr(runtime_instance_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(runtime_instance_id, '-', '')) = 32
            AND replace(runtime_instance_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    agent_step_no INTEGER NOT NULL CHECK (agent_step_no > 0),
    tool_ordinal INTEGER NOT NULL CHECK (tool_ordinal > 0),
    provider_tool_call_id TEXT NULL
        CHECK (provider_tool_call_id IS NULL OR (length(provider_tool_call_id) BETWEEN 1 AND 255 AND trim(provider_tool_call_id) = provider_tool_call_id)),
    tool_name TEXT NOT NULL
        CHECK (length(tool_name) BETWEEN 1 AND 64 AND tool_name GLOB '[a-z0-9]*' AND tool_name NOT GLOB '*[^a-z0-9._-]*'),
    tool_version TEXT NOT NULL CHECK (length(tool_version) BETWEEN 1 AND 64 AND trim(tool_version) = tool_version),
    tool_schema_version INTEGER NOT NULL CHECK (tool_schema_version > 0),
    arguments_json TEXT NOT NULL
        CHECK (length(arguments_json) <= 65536 AND json_valid(arguments_json) AND json_type(arguments_json) = 'object'),
    arguments_sha256 TEXT NOT NULL
        CHECK (length(arguments_sha256) = 64 AND arguments_sha256 NOT GLOB '*[^0-9a-f]*'),
    workstation_id TEXT NOT NULL
        CHECK (
            length(workstation_id) = 36
            AND substr(workstation_id, 9, 1) = '-'
            AND substr(workstation_id, 14, 1) = '-'
            AND substr(workstation_id, 19, 1) = '-'
            AND substr(workstation_id, 24, 1) = '-'
            AND substr(workstation_id, 15, 1) = '7'
            AND substr(workstation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(workstation_id, '-', '')) = 32
            AND replace(workstation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    workstation_generation INTEGER NOT NULL CHECK (workstation_generation > 0),
    workspace_id TEXT NOT NULL
        CHECK (
            length(workspace_id) = 36
            AND substr(workspace_id, 9, 1) = '-'
            AND substr(workspace_id, 14, 1) = '-'
            AND substr(workspace_id, 19, 1) = '-'
            AND substr(workspace_id, 24, 1) = '-'
            AND substr(workspace_id, 15, 1) = '7'
            AND substr(workspace_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(workspace_id, '-', '')) = 32
            AND replace(workspace_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    requested_cwd TEXT NOT NULL
        CHECK (
            length(requested_cwd) BETWEEN 1 AND 4096
            AND instr(requested_cwd, char(0)) = 0
            AND instr(requested_cwd, char(92)) = 0
        ),
    resolved_cwd TEXT NULL CHECK (resolved_cwd IS NULL OR (length(resolved_cwd) BETWEEN 1 AND 4096 AND instr(resolved_cwd, char(0)) = 0)),
    requested_privilege TEXT NOT NULL CHECK (requested_privilege IN ('user', 'administrative')),
    effective_privilege TEXT NULL CHECK (effective_privilege IS NULL OR effective_privilege IN ('user', 'administrative')),
    authority_decision_json TEXT NULL
        CHECK (authority_decision_json IS NULL OR (length(authority_decision_json) <= 16384 AND json_valid(authority_decision_json) AND json_type(authority_decision_json) = 'object')),
    timeout_ms INTEGER NOT NULL CHECK (timeout_ms BETWEEN 1 AND 900000),
    output_policy_json TEXT NOT NULL
        CHECK (length(output_policy_json) <= 16384 AND json_valid(output_policy_json) AND json_type(output_policy_json) = 'object'),
    state TEXT NOT NULL
        CHECK (state IN ('requested', 'dispatching', 'completed', 'interrupted_before_dispatch', 'outcome_unknown')),
    dispatch_intent_at TEXT NULL
        CHECK (
            dispatch_intent_at IS NULL
            OR (
                length(dispatch_intent_at) = 27
                AND dispatch_intent_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    requested_at TEXT NOT NULL
        CHECK (
            length(requested_at) = 27
            AND requested_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    started_at TEXT NULL
        CHECK (
            started_at IS NULL
            OR (
                length(started_at) = 27
                AND started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    completed_at TEXT NULL
        CHECK (
            completed_at IS NULL
            OR (
                length(completed_at) = 27
                AND completed_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    exit_code INTEGER NULL,
    signal INTEGER NULL CHECK (signal IS NULL OR signal > 0),
    timed_out INTEGER NULL CHECK (timed_out IS NULL OR timed_out IN (0, 1)),
    cancelled INTEGER NULL CHECK (cancelled IS NULL OR cancelled IN (0, 1)),
    cleanup_confirmed INTEGER NULL CHECK (cleanup_confirmed IS NULL OR cleanup_confirmed IN (0, 1)),
    result_json TEXT NULL
        CHECK (result_json IS NULL OR (length(result_json) <= 262144 AND json_valid(result_json) AND json_type(result_json) = 'object')),
    stdout_artifact_id TEXT NULL
        CHECK (
            stdout_artifact_id IS NULL
            OR (
                length(stdout_artifact_id) = 36
                AND substr(stdout_artifact_id, 9, 1) = '-'
                AND substr(stdout_artifact_id, 14, 1) = '-'
                AND substr(stdout_artifact_id, 19, 1) = '-'
                AND substr(stdout_artifact_id, 24, 1) = '-'
                AND substr(stdout_artifact_id, 15, 1) = '7'
                AND substr(stdout_artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(stdout_artifact_id, '-', '')) = 32
                AND replace(stdout_artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    stderr_artifact_id TEXT NULL
        CHECK (
            stderr_artifact_id IS NULL
            OR (
                length(stderr_artifact_id) = 36
                AND substr(stderr_artifact_id, 9, 1) = '-'
                AND substr(stderr_artifact_id, 14, 1) = '-'
                AND substr(stderr_artifact_id, 19, 1) = '-'
                AND substr(stderr_artifact_id, 24, 1) = '-'
                AND substr(stderr_artifact_id, 15, 1) = '7'
                AND substr(stderr_artifact_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(stderr_artifact_id, '-', '')) = 32
                AND replace(stderr_artifact_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    stdout_observed_bytes INTEGER NULL CHECK (stdout_observed_bytes IS NULL OR stdout_observed_bytes >= 0),
    stdout_captured_bytes INTEGER NULL CHECK (stdout_captured_bytes IS NULL OR stdout_captured_bytes >= 0),
    stdout_returned_inline_bytes INTEGER NULL CHECK (stdout_returned_inline_bytes IS NULL OR stdout_returned_inline_bytes >= 0),
    stdout_omitted_bytes INTEGER NULL CHECK (stdout_omitted_bytes IS NULL OR stdout_omitted_bytes >= 0),
    stderr_observed_bytes INTEGER NULL CHECK (stderr_observed_bytes IS NULL OR stderr_observed_bytes >= 0),
    stderr_captured_bytes INTEGER NULL CHECK (stderr_captured_bytes IS NULL OR stderr_captured_bytes >= 0),
    stderr_returned_inline_bytes INTEGER NULL CHECK (stderr_returned_inline_bytes IS NULL OR stderr_returned_inline_bytes >= 0),
    stderr_omitted_bytes INTEGER NULL CHECK (stderr_omitted_bytes IS NULL OR stderr_omitted_bytes >= 0),
    truncated INTEGER NOT NULL DEFAULT 0 CHECK (truncated IN (0, 1)),
    normalized_error_json TEXT NULL
        CHECK (normalized_error_json IS NULL OR (length(normalized_error_json) <= 65536 AND json_valid(normalized_error_json) AND json_type(normalized_error_json) = 'object')),
    CHECK (dispatch_intent_at IS NULL OR dispatch_intent_at >= requested_at),
    CHECK (started_at IS NULL OR (dispatch_intent_at IS NOT NULL AND started_at >= dispatch_intent_at)),
    CHECK (completed_at IS NULL OR completed_at >= requested_at),
    CHECK (
        (stdout_observed_bytes IS NULL AND stdout_captured_bytes IS NULL AND stdout_returned_inline_bytes IS NULL AND stdout_omitted_bytes IS NULL)
        OR (
            stdout_observed_bytes IS NOT NULL
            AND stdout_captured_bytes IS NOT NULL
            AND stdout_returned_inline_bytes IS NOT NULL
            AND stdout_omitted_bytes IS NOT NULL
            AND stdout_observed_bytes >= stdout_captured_bytes
            AND stdout_captured_bytes >= stdout_returned_inline_bytes
            AND stdout_omitted_bytes = stdout_observed_bytes - stdout_returned_inline_bytes
            AND (stdout_artifact_id IS NOT NULL OR stdout_captured_bytes = 0)
        )
    ),
    CHECK (
        (stderr_observed_bytes IS NULL AND stderr_captured_bytes IS NULL AND stderr_returned_inline_bytes IS NULL AND stderr_omitted_bytes IS NULL)
        OR (
            stderr_observed_bytes IS NOT NULL
            AND stderr_captured_bytes IS NOT NULL
            AND stderr_returned_inline_bytes IS NOT NULL
            AND stderr_omitted_bytes IS NOT NULL
            AND stderr_observed_bytes >= stderr_captured_bytes
            AND stderr_captured_bytes >= stderr_returned_inline_bytes
            AND stderr_omitted_bytes = stderr_observed_bytes - stderr_returned_inline_bytes
            AND (stderr_artifact_id IS NOT NULL OR stderr_captured_bytes = 0)
        )
    ),
    CHECK (stdout_artifact_id IS NULL OR stdout_observed_bytes IS NOT NULL),
    CHECK (stderr_artifact_id IS NULL OR stderr_observed_bytes IS NOT NULL),
    CHECK (
        truncated = (
            coalesce(stdout_observed_bytes > stdout_captured_bytes, 0)
            OR coalesce(stderr_observed_bytes > stderr_captured_bytes, 0)
        )
    ),
    CHECK (
        (
            state = 'requested'
            AND dispatch_intent_at IS NULL
            AND started_at IS NULL
            AND completed_at IS NULL
            AND resolved_cwd IS NULL
            AND effective_privilege IS NULL
            AND authority_decision_json IS NULL
            AND exit_code IS NULL
            AND signal IS NULL
            AND timed_out IS NULL
            AND cancelled IS NULL
            AND cleanup_confirmed IS NULL
            AND result_json IS NULL
            AND stdout_artifact_id IS NULL
            AND stderr_artifact_id IS NULL
            AND stdout_observed_bytes IS NULL
            AND stderr_observed_bytes IS NULL
            AND truncated = 0
            AND normalized_error_json IS NULL
        )
        OR (
            state = 'dispatching'
            AND dispatch_intent_at IS NOT NULL
            AND completed_at IS NULL
            AND resolved_cwd IS NOT NULL
            AND effective_privilege IS NOT NULL
            AND authority_decision_json IS NOT NULL
            AND json_extract(authority_decision_json, '$.decision') = 'allow'
            AND json_extract(authority_decision_json, '$.effective_privilege') = effective_privilege
            AND json_extract(authority_decision_json, '$.policy') = 'v0-development-workstation'
            AND exit_code IS NULL
            AND signal IS NULL
            AND timed_out IS NULL
            AND cancelled IS NULL
            AND cleanup_confirmed IS NULL
            AND result_json IS NULL
            AND stdout_artifact_id IS NULL
            AND stderr_artifact_id IS NULL
            AND stdout_observed_bytes IS NULL
            AND stderr_observed_bytes IS NULL
            AND truncated = 0
            AND normalized_error_json IS NULL
        )
        OR (
            state = 'completed'
            AND completed_at IS NOT NULL
            AND result_json IS NOT NULL
            AND (
                (
                    dispatch_intent_at IS NULL
                    AND started_at IS NULL
                    AND resolved_cwd IS NULL
                    AND effective_privilege IS NULL
                    AND (
                        authority_decision_json IS NULL
                        OR (
                            json_extract(authority_decision_json, '$.decision') = 'deny'
                            AND json_extract(result_json, '$.result_kind') = 'authority_denial'
                        )
                    )
                    AND json_extract(result_json, '$.result_kind') IN (
                        'validation_rejection',
                        'unknown_tool',
                        'authority_denial',
                        'file_error',
                        'cancellation'
                    )
                    AND exit_code IS NULL
                    AND signal IS NULL
                    AND timed_out IS NULL
                    AND cancelled IS NULL
                    AND cleanup_confirmed IS NULL
                    AND stdout_artifact_id IS NULL
                    AND stderr_artifact_id IS NULL
                    AND stdout_observed_bytes IS NULL
                    AND stdout_captured_bytes IS NULL
                    AND stdout_returned_inline_bytes IS NULL
                    AND stdout_omitted_bytes IS NULL
                    AND stderr_observed_bytes IS NULL
                    AND stderr_captured_bytes IS NULL
                    AND stderr_returned_inline_bytes IS NULL
                    AND stderr_omitted_bytes IS NULL
                    AND truncated = 0
                )
                OR (
                    dispatch_intent_at IS NOT NULL
                    AND resolved_cwd IS NOT NULL
                    AND effective_privilege IS NOT NULL
                    AND authority_decision_json IS NOT NULL
                    AND json_extract(authority_decision_json, '$.decision') = 'allow'
                    AND json_extract(authority_decision_json, '$.effective_privilege') = effective_privilege
                    AND json_extract(authority_decision_json, '$.policy') = 'v0-development-workstation'
                )
            )
        )
        OR (
            state = 'interrupted_before_dispatch'
            AND dispatch_intent_at IS NULL
            AND started_at IS NULL
            AND completed_at IS NOT NULL
            AND resolved_cwd IS NULL
            AND effective_privilege IS NULL
            AND authority_decision_json IS NULL
            AND exit_code IS NULL
            AND signal IS NULL
            AND timed_out IS NULL
            AND cancelled IS NULL
            AND cleanup_confirmed IS NULL
            AND result_json IS NULL
            AND stdout_artifact_id IS NULL
            AND stderr_artifact_id IS NULL
            AND stdout_observed_bytes IS NULL
            AND stderr_observed_bytes IS NULL
            AND truncated = 0
            AND normalized_error_json IS NOT NULL
        )
        OR (
            state = 'outcome_unknown'
            AND dispatch_intent_at IS NOT NULL
            AND completed_at IS NOT NULL
            AND resolved_cwd IS NOT NULL
            AND effective_privilege IS NOT NULL
            AND authority_decision_json IS NOT NULL
            AND json_extract(authority_decision_json, '$.decision') = 'allow'
            AND cleanup_confirmed = 0
            AND result_json IS NULL
            AND stdout_artifact_id IS NULL
            AND stderr_artifact_id IS NULL
            AND stdout_observed_bytes IS NULL
            AND stderr_observed_bytes IS NULL
            AND truncated = 0
            AND normalized_error_json IS NOT NULL
        )
    ),
    FOREIGN KEY (work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (source_model_invocation_id) REFERENCES model_invocations (model_invocation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (runtime_instance_id) REFERENCES runtime_instances (runtime_instance_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (workstation_id) REFERENCES workstations (workstation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id) REFERENCES workspaces (workspace_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (stdout_artifact_id) REFERENCES artifacts (artifact_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (stderr_artifact_id) REFERENCES artifacts (artifact_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_tool_executions_execution_id
    ON tool_executions (execution_id);
CREATE UNIQUE INDEX ux_tool_executions_work_step_ordinal
    ON tool_executions (work_id, agent_step_no, tool_ordinal);
CREATE UNIQUE INDEX ux_tool_executions_source_ordinal
    ON tool_executions (source_model_invocation_id, tool_ordinal);
CREATE UNIQUE INDEX ux_tool_executions_source_provider_call
    ON tool_executions (source_model_invocation_id, provider_tool_call_id)
    WHERE provider_tool_call_id IS NOT NULL;
CREATE UNIQUE INDEX ux_tool_executions_one_nonterminal_per_work
    ON tool_executions (work_id)
    WHERE state IN ('requested', 'dispatching');
CREATE INDEX ix_tool_executions_runtime_nonterminal
    ON tool_executions (runtime_instance_id, state, work_id)
    WHERE state IN ('requested', 'dispatching');
