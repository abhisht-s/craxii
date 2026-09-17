CREATE TABLE outbound_deliveries (
    outbound_delivery_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(outbound_delivery_id) = 36
            AND substr(outbound_delivery_id, 9, 1) = '-'
            AND substr(outbound_delivery_id, 14, 1) = '-'
            AND substr(outbound_delivery_id, 19, 1) = '-'
            AND substr(outbound_delivery_id, 24, 1) = '-'
            AND substr(outbound_delivery_id, 15, 1) = '7'
            AND substr(outbound_delivery_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(outbound_delivery_id, '-', '')) = 32
            AND replace(outbound_delivery_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    craxii_id TEXT NOT NULL,
    conversation_binding_id TEXT NOT NULL,
    channel_account_id TEXT NOT NULL,
    provider_key TEXT NOT NULL CHECK (
        length(CAST(provider_key AS BLOB)) BETWEEN 1 AND 64
        AND provider_key NOT GLOB '*[^a-z0-9._-]*'
    ),
    external_conversation_id TEXT NOT NULL CHECK (
        length(CAST(external_conversation_id AS BLOB)) BETWEEN 1 AND 255
        AND external_conversation_id = trim(external_conversation_id)
    ),
    external_thread_id TEXT NULL CHECK (
        external_thread_id IS NULL OR (
            length(CAST(external_thread_id AS BLOB)) BETWEEN 1 AND 255
            AND external_thread_id = trim(external_thread_id)
        )
    ),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('assistant', 'control')),
    source_message_id TEXT NULL,
    source_work_id TEXT NULL,
    source_inbound_delivery_id TEXT NULL,
    control_outcome TEXT NULL CHECK (control_outcome IS NULL OR control_outcome IN ('applied', 'no_op')),
    payload_version INTEGER NOT NULL CHECK (payload_version = 1),
    payload_text TEXT NOT NULL CHECK (length(CAST(payload_text AS BLOB)) >= 1),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    part_ordinal INTEGER NOT NULL,
    part_count INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'dispatching', 'retry_wait', 'accepted', 'permanent_failure', 'outcome_unknown'
    )),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 8),
    dispatch_runtime_instance_id TEXT NULL,
    next_attempt_at TEXT NULL CHECK (next_attempt_at IS NULL OR (
        length(next_attempt_at) = 27
        AND next_attempt_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    delivery_deadline_at TEXT NOT NULL CHECK (
        length(delivery_deadline_at) = 27
        AND delivery_deadline_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    accepted_external_message_id TEXT NULL CHECK (
        accepted_external_message_id IS NULL OR (
            length(CAST(accepted_external_message_id AS BLOB)) BETWEEN 1 AND 255
            AND accepted_external_message_id = trim(accepted_external_message_id)
        )
    ),
    failure_class TEXT NULL CHECK (failure_class IS NULL OR failure_class IN (
        'adapter_unavailable', 'binding_revoked', 'channel_account_disabled',
        'unsupported_payload', 'profile_unavailable', 'payload_too_large',
        'prior_part_permanent_failure', 'prior_part_outcome_unknown', 'retry_exhausted',
        'delivery_deadline_exceeded', 'provider_retryable', 'provider_permanent',
        'provider_outcome_unknown', 'stale_dispatch', 'shutdown_interrupted',
        'storage_inconsistent'
    )),
    failure_code TEXT NULL CHECK (
        failure_code IS NULL OR (
            length(CAST(failure_code AS BLOB)) BETWEEN 1 AND 64
            AND failure_code NOT GLOB '*[^a-z0-9._-]*'
        )
    ),
    created_at TEXT NOT NULL CHECK (
        length(created_at) = 27
        AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    updated_at TEXT NOT NULL CHECK (
        length(updated_at) = 27
        AND updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    accepted_at TEXT NULL CHECK (accepted_at IS NULL OR (
        length(accepted_at) = 27
        AND accepted_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    terminal_at TEXT NULL CHECK (terminal_at IS NULL OR (
        length(terminal_at) = 27
        AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    CHECK (part_ordinal BETWEEN 1 AND part_count AND part_count BETWEEN 1 AND 64),
    CHECK (delivery_deadline_at > created_at AND updated_at >= created_at),
    CHECK (
        (source_kind = 'assistant'
            AND source_message_id IS NOT NULL
            AND source_work_id IS NOT NULL
            AND source_inbound_delivery_id IS NULL
            AND control_outcome IS NULL)
        OR
        (source_kind = 'control'
            AND source_message_id IS NULL
            AND source_work_id IS NULL
            AND source_inbound_delivery_id IS NOT NULL
            AND control_outcome IN ('applied', 'no_op'))
    ),
    CHECK (failure_code IS NULL OR failure_class IS NOT NULL),
    CHECK (
        (state = 'queued'
            AND attempt_count = 0
            AND dispatch_runtime_instance_id IS NULL
            AND next_attempt_at IS NOT NULL
            AND accepted_external_message_id IS NULL
            AND failure_class IS NULL AND failure_code IS NULL
            AND accepted_at IS NULL AND terminal_at IS NULL)
        OR
        (state = 'dispatching'
            AND attempt_count BETWEEN 1 AND 8
            AND dispatch_runtime_instance_id IS NOT NULL
            AND next_attempt_at IS NULL
            AND accepted_external_message_id IS NULL
            AND failure_class IS NULL AND failure_code IS NULL
            AND accepted_at IS NULL AND terminal_at IS NULL)
        OR
        (state = 'retry_wait'
            AND attempt_count BETWEEN 1 AND 7
            AND dispatch_runtime_instance_id IS NULL
            AND next_attempt_at IS NOT NULL
            AND accepted_external_message_id IS NULL
            AND failure_class IS NULL AND failure_code IS NULL
            AND accepted_at IS NULL AND terminal_at IS NULL)
        OR
        (state = 'accepted'
            AND attempt_count BETWEEN 1 AND 8
            AND dispatch_runtime_instance_id IS NULL
            AND next_attempt_at IS NULL
            AND failure_class IS NULL AND failure_code IS NULL
            AND accepted_at IS NOT NULL AND terminal_at = accepted_at)
        OR
        (state = 'permanent_failure'
            AND dispatch_runtime_instance_id IS NULL
            AND next_attempt_at IS NULL
            AND accepted_external_message_id IS NULL
            AND failure_class IS NOT NULL
            AND accepted_at IS NULL AND terminal_at IS NOT NULL)
        OR
        (state = 'outcome_unknown'
            AND attempt_count BETWEEN 1 AND 8
            AND dispatch_runtime_instance_id IS NULL
            AND next_attempt_at IS NULL
            AND accepted_external_message_id IS NULL
            AND failure_class IS NOT NULL
            AND accepted_at IS NULL AND terminal_at IS NOT NULL)
    ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_binding_id) REFERENCES conversation_bindings (conversation_binding_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (channel_account_id, craxii_id)
        REFERENCES channel_accounts (channel_account_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (source_message_id) REFERENCES messages (message_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (source_work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (source_inbound_delivery_id) REFERENCES inbound_deliveries (inbound_delivery_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (dispatch_runtime_instance_id) REFERENCES runtime_instances (runtime_instance_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_outbound_deliveries_assistant_source_part
    ON outbound_deliveries (source_message_id, part_ordinal)
    WHERE source_kind = 'assistant';

CREATE UNIQUE INDEX ux_outbound_deliveries_control_source_part
    ON outbound_deliveries (source_inbound_delivery_id, part_ordinal)
    WHERE source_kind = 'control';

CREATE UNIQUE INDEX ux_outbound_deliveries_account_provider_message
    ON outbound_deliveries (channel_account_id, accepted_external_message_id)
    WHERE accepted_external_message_id IS NOT NULL;

CREATE INDEX ix_outbound_deliveries_due
    ON outbound_deliveries (next_attempt_at, created_at, outbound_delivery_id)
    WHERE state IN ('queued', 'retry_wait');

CREATE INDEX ix_outbound_deliveries_state_updated
    ON outbound_deliveries (state, updated_at, outbound_delivery_id);

CREATE TABLE outbound_delivery_attempts (
    outbound_delivery_attempt_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(outbound_delivery_attempt_id) = 36
            AND substr(outbound_delivery_attempt_id, 9, 1) = '-'
            AND substr(outbound_delivery_attempt_id, 14, 1) = '-'
            AND substr(outbound_delivery_attempt_id, 19, 1) = '-'
            AND substr(outbound_delivery_attempt_id, 24, 1) = '-'
            AND substr(outbound_delivery_attempt_id, 15, 1) = '7'
            AND substr(outbound_delivery_attempt_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(outbound_delivery_attempt_id, '-', '')) = 32
            AND replace(outbound_delivery_attempt_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    outbound_delivery_id TEXT NOT NULL,
    runtime_instance_id TEXT NOT NULL,
    attempt_number INTEGER NOT NULL CHECK (attempt_number BETWEEN 1 AND 8),
    prior_state TEXT NOT NULL CHECK (prior_state IN ('queued', 'retry_wait')),
    dispatch_material_version INTEGER NOT NULL CHECK (dispatch_material_version = 1),
    dispatch_material_sha256 TEXT NOT NULL CHECK (
        length(dispatch_material_sha256) = 64
        AND dispatch_material_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    started_at TEXT NOT NULL CHECK (
        length(started_at) = 27
        AND started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    completed_at TEXT NULL CHECK (completed_at IS NULL OR (
        length(completed_at) = 27
        AND completed_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    result_kind TEXT NULL CHECK (result_kind IS NULL OR result_kind IN (
        'accepted', 'retryable_failure', 'permanent_failure', 'outcome_unknown'
    )),
    failure_class TEXT NULL CHECK (failure_class IS NULL OR failure_class IN (
        'adapter_unavailable', 'binding_revoked', 'channel_account_disabled',
        'unsupported_payload', 'profile_unavailable', 'payload_too_large',
        'prior_part_permanent_failure', 'prior_part_outcome_unknown', 'retry_exhausted',
        'delivery_deadline_exceeded', 'provider_retryable', 'provider_permanent',
        'provider_outcome_unknown', 'stale_dispatch', 'shutdown_interrupted',
        'storage_inconsistent'
    )),
    failure_code TEXT NULL CHECK (
        failure_code IS NULL OR (
            length(CAST(failure_code AS BLOB)) BETWEEN 1 AND 64
            AND failure_code NOT GLOB '*[^a-z0-9._-]*'
        )
    ),
    provider_retry_after_ms INTEGER NULL CHECK (provider_retry_after_ms IS NULL OR provider_retry_after_ms >= 0),
    selected_retry_delay_ms INTEGER NULL CHECK (selected_retry_delay_ms IS NULL OR selected_retry_delay_ms >= 1000),
    scheduled_next_attempt_at TEXT NULL CHECK (scheduled_next_attempt_at IS NULL OR (
        length(scheduled_next_attempt_at) = 27
        AND scheduled_next_attempt_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    accepted_external_message_id TEXT NULL CHECK (
        accepted_external_message_id IS NULL OR (
            length(CAST(accepted_external_message_id AS BLOB)) BETWEEN 1 AND 255
            AND accepted_external_message_id = trim(accepted_external_message_id)
        )
    ),
    CHECK (completed_at IS NULL OR completed_at >= started_at),
    CHECK (failure_code IS NULL OR failure_class IS NOT NULL),
    CHECK (
        (completed_at IS NULL
            AND result_kind IS NULL AND failure_class IS NULL AND failure_code IS NULL
            AND provider_retry_after_ms IS NULL AND selected_retry_delay_ms IS NULL
            AND scheduled_next_attempt_at IS NULL AND accepted_external_message_id IS NULL)
        OR
        (completed_at IS NOT NULL AND (
            (result_kind = 'accepted'
                AND failure_class IS NULL AND failure_code IS NULL
                AND provider_retry_after_ms IS NULL AND selected_retry_delay_ms IS NULL
                AND scheduled_next_attempt_at IS NULL)
            OR
            (result_kind = 'retryable_failure'
                AND failure_class IS NOT NULL
                AND selected_retry_delay_ms IS NOT NULL
                AND accepted_external_message_id IS NULL)
            OR
            (result_kind IN ('permanent_failure', 'outcome_unknown')
                AND failure_class IS NOT NULL
                AND provider_retry_after_ms IS NULL AND selected_retry_delay_ms IS NULL
                AND scheduled_next_attempt_at IS NULL AND accepted_external_message_id IS NULL)
        ))
    ),
    FOREIGN KEY (outbound_delivery_id) REFERENCES outbound_deliveries (outbound_delivery_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (runtime_instance_id) REFERENCES runtime_instances (runtime_instance_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_outbound_delivery_attempts_delivery_number
    ON outbound_delivery_attempts (outbound_delivery_id, attempt_number);

CREATE UNIQUE INDEX ux_outbound_delivery_attempts_one_open
    ON outbound_delivery_attempts (outbound_delivery_id)
    WHERE completed_at IS NULL;

CREATE INDEX ix_outbound_delivery_attempts_runtime_open
    ON outbound_delivery_attempts (runtime_instance_id, started_at)
    WHERE completed_at IS NULL;
