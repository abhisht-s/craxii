CREATE TABLE craxii_principals (
    craxii_id TEXT PRIMARY KEY NOT NULL
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
    display_name TEXT NOT NULL,
    owner_label TEXT NOT NULL,
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state = 'active'),
    primary_conversation_id TEXT NULL
        CHECK (
            primary_conversation_id IS NULL
            OR (
                length(primary_conversation_id) = 36
                AND substr(primary_conversation_id, 9, 1) = '-'
                AND substr(primary_conversation_id, 14, 1) = '-'
                AND substr(primary_conversation_id, 19, 1) = '-'
                AND substr(primary_conversation_id, 24, 1) = '-'
                AND substr(primary_conversation_id, 15, 1) = '7'
                AND substr(primary_conversation_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(primary_conversation_id, '-', '')) = 32
                AND replace(primary_conversation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    default_workspace_id TEXT NULL
        CHECK (
            default_workspace_id IS NULL
            OR (
                length(default_workspace_id) = 36
                AND substr(default_workspace_id, 9, 1) = '-'
                AND substr(default_workspace_id, 14, 1) = '-'
                AND substr(default_workspace_id, 19, 1) = '-'
                AND substr(default_workspace_id, 24, 1) = '-'
                AND substr(default_workspace_id, 15, 1) = '7'
                AND substr(default_workspace_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(default_workspace_id, '-', '')) = 32
                AND replace(default_workspace_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    architecture_revision TEXT NOT NULL CHECK (length(architecture_revision) > 0),
    schema_revision INTEGER NOT NULL CHECK (schema_revision > 0),
    CHECK (
        (primary_conversation_id IS NULL AND default_workspace_id IS NULL)
        OR (primary_conversation_id IS NOT NULL AND default_workspace_id IS NOT NULL)
    ),
    FOREIGN KEY (primary_conversation_id) REFERENCES conversations (conversation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (default_workspace_id) REFERENCES workspaces (workspace_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE TABLE workstations (
    workstation_id TEXT PRIMARY KEY NOT NULL
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
    kind TEXT NOT NULL CHECK (kind = 'local'),
    generation INTEGER NOT NULL CHECK (generation > 0),
    hosting_provider TEXT NOT NULL CHECK (length(hosting_provider) > 0),
    provider_instance_id TEXT NULL,
    provider_image_id TEXT NULL,
    provisioning_revision TEXT NULL,
    architecture TEXT NOT NULL CHECK (length(architecture) > 0),
    os_release TEXT NOT NULL CHECK (length(os_release) > 0),
    capabilities_json TEXT NOT NULL CHECK (json_valid(capabilities_json)),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    last_seen_at TEXT NOT NULL
        CHECK (
            length(last_seen_at) = 27
            AND last_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_workstations_craxii_id
    ON workstations (craxii_id);

CREATE TABLE workspaces (
    workspace_id TEXT PRIMARY KEY NOT NULL
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
    logical_name TEXT NOT NULL CHECK (length(logical_name) > 0),
    logical_root TEXT NOT NULL CHECK (substr(logical_root, 1, 1) = '/'),
    local_resolved_root TEXT NOT NULL CHECK (substr(local_resolved_root, 1, 1) = '/'),
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state = 'active'),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (workstation_id) REFERENCES workstations (workstation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_workspaces_workstation_logical_name
    ON workspaces (workstation_id, logical_name);
CREATE INDEX ix_workspaces_craxii_id
    ON workspaces (craxii_id);

CREATE TABLE conversations (
    conversation_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(conversation_id) = 36
            AND substr(conversation_id, 9, 1) = '-'
            AND substr(conversation_id, 14, 1) = '-'
            AND substr(conversation_id, 19, 1) = '-'
            AND substr(conversation_id, 24, 1) = '-'
            AND substr(conversation_id, 15, 1) = '7'
            AND substr(conversation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(conversation_id, '-', '')) = 32
            AND replace(conversation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
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
    kind TEXT NOT NULL CHECK (kind = 'primary'),
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state = 'active'),
    next_work_ordinal INTEGER NOT NULL DEFAULT 1 CHECK (next_work_ordinal > 0),
    state_version INTEGER NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_conversations_craxii_kind
    ON conversations (craxii_id, kind);

CREATE TABLE runtime_instances (
    runtime_instance_id TEXT PRIMARY KEY NOT NULL
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
    linux_boot_id TEXT NOT NULL CHECK (length(linux_boot_id) > 0),
    process_id INTEGER NOT NULL CHECK (process_id > 0),
    binary_version TEXT NOT NULL CHECK (length(binary_version) > 0),
    git_revision TEXT NOT NULL CHECK (length(git_revision) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    state TEXT NOT NULL CHECK (state IN ('running', 'stopping', 'stopped')),
    started_at TEXT NOT NULL
        CHECK (
            length(started_at) = 27
            AND started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    last_heartbeat_at TEXT NULL
        CHECK (
            last_heartbeat_at IS NULL
            OR (
                length(last_heartbeat_at) = 27
                AND last_heartbeat_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    stopped_at TEXT NULL
        CHECK (
            stopped_at IS NULL
            OR (
                length(stopped_at) = 27
                AND stopped_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    stop_reason TEXT NULL CHECK (stop_reason IS NULL OR stop_reason IN ('graceful_shutdown', 'startup_failure')),
    CHECK (
        (state = 'running' AND stopped_at IS NULL AND stop_reason IS NULL)
        OR (
            state = 'stopping'
            AND stopped_at IS NULL
            AND (stop_reason IS NULL OR stop_reason = 'graceful_shutdown')
        )
        OR (
            state = 'stopped'
            AND stopped_at IS NOT NULL
            AND stop_reason IS NOT NULL
            AND stop_reason IN ('graceful_shutdown', 'startup_failure')
        )
    ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (workstation_id) REFERENCES workstations (workstation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_runtime_instances_craxii_state
    ON runtime_instances (craxii_id, state);

CREATE TABLE client_devices (
    device_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(device_id) = 36
            AND substr(device_id, 9, 1) = '-'
            AND substr(device_id, 14, 1) = '-'
            AND substr(device_id, 19, 1) = '-'
            AND substr(device_id, 24, 1) = '-'
            AND substr(device_id, 15, 1) = '7'
            AND substr(device_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(device_id, '-', '')) = 32
            AND replace(device_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    display_name TEXT NOT NULL,
    token_hash TEXT NOT NULL
        CHECK (length(token_hash) = 64 AND token_hash NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    last_seen_at TEXT NULL
        CHECK (
            last_seen_at IS NULL
            OR (
                length(last_seen_at) = 27
                AND last_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    revoked_at TEXT NULL
        CHECK (
            revoked_at IS NULL
            OR (
                length(revoked_at) = 27
                AND revoked_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        )
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_client_devices_token_hash
    ON client_devices (token_hash);

CREATE TABLE work_items (
    work_id TEXT PRIMARY KEY NOT NULL
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
    conversation_id TEXT NOT NULL
        CHECK (
            length(conversation_id) = 36
            AND substr(conversation_id, 9, 1) = '-'
            AND substr(conversation_id, 14, 1) = '-'
            AND substr(conversation_id, 19, 1) = '-'
            AND substr(conversation_id, 24, 1) = '-'
            AND substr(conversation_id, 15, 1) = '7'
            AND substr(conversation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(conversation_id, '-', '')) = 32
            AND replace(conversation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    conversation_work_ordinal INTEGER NOT NULL CHECK (conversation_work_ordinal > 0),
    kind TEXT NOT NULL CHECK (kind = 'conversational'),
    state TEXT NOT NULL
        CHECK (
            state IN (
                'queued',
                'running',
                'waiting_on_model',
                'waiting_on_tool',
                'cancel_requested',
                'completed',
                'failed',
                'cancelled',
                'interrupted'
            )
        ),
    state_version INTEGER NOT NULL DEFAULT 1 CHECK (state_version > 0),
    priority INTEGER NOT NULL DEFAULT 0 CHECK (priority = 0),
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
    runtime_instance_id TEXT NULL
        CHECK (
            runtime_instance_id IS NULL
            OR (
                length(runtime_instance_id) = 36
                AND substr(runtime_instance_id, 9, 1) = '-'
                AND substr(runtime_instance_id, 14, 1) = '-'
                AND substr(runtime_instance_id, 19, 1) = '-'
                AND substr(runtime_instance_id, 24, 1) = '-'
                AND substr(runtime_instance_id, 15, 1) = '7'
                AND substr(runtime_instance_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(runtime_instance_id, '-', '')) = 32
                AND replace(runtime_instance_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    current_model_invocation_id TEXT NULL
        CHECK (
            current_model_invocation_id IS NULL
            OR (
                length(current_model_invocation_id) = 36
                AND substr(current_model_invocation_id, 9, 1) = '-'
                AND substr(current_model_invocation_id, 14, 1) = '-'
                AND substr(current_model_invocation_id, 19, 1) = '-'
                AND substr(current_model_invocation_id, 24, 1) = '-'
                AND substr(current_model_invocation_id, 15, 1) = '7'
                AND substr(current_model_invocation_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(current_model_invocation_id, '-', '')) = 32
                AND replace(current_model_invocation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    current_tool_execution_id TEXT NULL
        CHECK (
            current_tool_execution_id IS NULL
            OR (
                length(current_tool_execution_id) = 36
                AND substr(current_tool_execution_id, 9, 1) = '-'
                AND substr(current_tool_execution_id, 14, 1) = '-'
                AND substr(current_tool_execution_id, 19, 1) = '-'
                AND substr(current_tool_execution_id, 24, 1) = '-'
                AND substr(current_tool_execution_id, 15, 1) = '7'
                AND substr(current_tool_execution_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(current_tool_execution_id, '-', '')) = 32
                AND replace(current_tool_execution_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    correlation_id TEXT NOT NULL
        CHECK (
            length(correlation_id) = 36
            AND substr(correlation_id, 9, 1) = '-'
            AND substr(correlation_id, 14, 1) = '-'
            AND substr(correlation_id, 19, 1) = '-'
            AND substr(correlation_id, 24, 1) = '-'
            AND substr(correlation_id, 15, 1) = '7'
            AND substr(correlation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(correlation_id, '-', '')) = 32
            AND replace(correlation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    queued_at TEXT NOT NULL
        CHECK (
            length(queued_at) = 27
            AND queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    started_at TEXT NULL
        CHECK (
            started_at IS NULL
            OR (
                length(started_at) = 27
                AND started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    cancel_requested_at TEXT NULL
        CHECK (
            cancel_requested_at IS NULL
            OR (
                length(cancel_requested_at) = 27
                AND cancel_requested_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    cancellation_reason_code TEXT NULL
        CHECK (
            cancellation_reason_code IS NULL
            OR cancellation_reason_code IN ('user_request', 'graceful_shutdown')
        ),
    terminal_at TEXT NULL
        CHECK (
            terminal_at IS NULL
            OR (
                length(terminal_at) = 27
                AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    terminal_reason_code TEXT NULL
        CHECK (
            terminal_reason_code IS NULL
            OR terminal_reason_code IN (
                'answered',
                'refused',
                'definite_normalized_error',
                'provider_exhausted',
                'invalid_model_output',
                'lifecycle_limit',
                'user_request',
                'graceful_shutdown',
                'runtime_ownership_lost',
                'provider_outcome_unknown',
                'tool_interrupted_before_dispatch',
                'tool_outcome_unknown',
                'cleanup_unconfirmed'
            )
        ),
    terminal_detail_json TEXT NULL
        CHECK (terminal_detail_json IS NULL OR json_valid(terminal_detail_json)),
    CHECK (current_model_invocation_id IS NULL OR current_tool_execution_id IS NULL),
    CHECK (
        (
            state = 'queued'
            AND runtime_instance_id IS NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NULL
            AND started_at IS NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'running'
            AND runtime_instance_id IS NOT NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'waiting_on_model'
            AND runtime_instance_id IS NOT NULL
            AND current_model_invocation_id IS NOT NULL
            AND current_tool_execution_id IS NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'waiting_on_tool'
            AND runtime_instance_id IS NOT NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NOT NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'cancel_requested'
            AND runtime_instance_id IS NOT NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NOT NULL
            AND cancellation_reason_code IS NOT NULL
            AND cancellation_reason_code IN ('user_request', 'graceful_shutdown')
            AND terminal_at IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'completed'
            AND runtime_instance_id IS NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NOT NULL
            AND terminal_reason_code IS NOT NULL
            AND terminal_reason_code IN ('answered', 'refused')
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'failed'
            AND runtime_instance_id IS NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NOT NULL
            AND terminal_reason_code IS NOT NULL
            AND terminal_reason_code IN (
                'definite_normalized_error',
                'provider_exhausted',
                'invalid_model_output',
                'lifecycle_limit'
            )
            AND (
                (terminal_reason_code = 'provider_exhausted' AND terminal_detail_json IS NULL)
                OR (terminal_reason_code <> 'provider_exhausted' AND terminal_detail_json IS NOT NULL)
            )
        )
        OR (
            state = 'cancelled'
            AND runtime_instance_id IS NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NOT NULL
            AND terminal_reason_code IS NOT NULL
            AND terminal_reason_code IN ('user_request', 'graceful_shutdown')
            AND terminal_detail_json IS NULL
        )
        OR (
            state = 'interrupted'
            AND runtime_instance_id IS NULL
            AND current_model_invocation_id IS NULL
            AND current_tool_execution_id IS NULL
            AND started_at IS NOT NULL
            AND cancel_requested_at IS NULL
            AND cancellation_reason_code IS NULL
            AND terminal_at IS NOT NULL
            AND terminal_reason_code IS NOT NULL
            AND terminal_reason_code IN (
                'runtime_ownership_lost',
                'provider_outcome_unknown',
                'tool_interrupted_before_dispatch',
                'tool_outcome_unknown',
                'cleanup_unconfirmed'
            )
            AND terminal_detail_json IS NULL
        )
    ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id) REFERENCES conversations (conversation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id) REFERENCES workspaces (workspace_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (runtime_instance_id) REFERENCES runtime_instances (runtime_instance_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_work_items_conversation_ordinal
    ON work_items (conversation_id, conversation_work_ordinal);
CREATE UNIQUE INDEX ux_work_items_one_active_per_conversation
    ON work_items (conversation_id)
    WHERE state IN ('running', 'waiting_on_model', 'waiting_on_tool', 'cancel_requested');
CREATE INDEX ix_work_items_queued_fifo
    ON work_items (conversation_id, state, conversation_work_ordinal, work_id);
CREATE INDEX ix_work_items_nonterminal_by_runtime
    ON work_items (runtime_instance_id, state, conversation_id, conversation_work_ordinal)
    WHERE runtime_instance_id IS NOT NULL
        AND state IN ('running', 'waiting_on_model', 'waiting_on_tool', 'cancel_requested');
CREATE UNIQUE INDEX ux_work_items_current_model_invocation
    ON work_items (current_model_invocation_id)
    WHERE current_model_invocation_id IS NOT NULL;
CREATE UNIQUE INDEX ux_work_items_current_tool_execution
    ON work_items (current_tool_execution_id)
    WHERE current_tool_execution_id IS NOT NULL;

CREATE TABLE messages (
    message_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(message_id) = 36
            AND substr(message_id, 9, 1) = '-'
            AND substr(message_id, 14, 1) = '-'
            AND substr(message_id, 19, 1) = '-'
            AND substr(message_id, 24, 1) = '-'
            AND substr(message_id, 15, 1) = '7'
            AND substr(message_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(message_id, '-', '')) = 32
            AND replace(message_id, '-', '') NOT GLOB '*[^0-9a-f]*'
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
    conversation_id TEXT NOT NULL
        CHECK (
            length(conversation_id) = 36
            AND substr(conversation_id, 9, 1) = '-'
            AND substr(conversation_id, 14, 1) = '-'
            AND substr(conversation_id, 19, 1) = '-'
            AND substr(conversation_id, 24, 1) = '-'
            AND substr(conversation_id, 15, 1) = '7'
            AND substr(conversation_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(conversation_id, '-', '')) = 32
            AND replace(conversation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content_json TEXT NOT NULL CHECK (json_valid(content_json)),
    content_sha256 TEXT NOT NULL
        CHECK (length(content_sha256) = 64 AND content_sha256 NOT GLOB '*[^0-9a-f]*'),
    produced_by_work_id TEXT NULL
        CHECK (
            produced_by_work_id IS NULL
            OR (
                length(produced_by_work_id) = 36
                AND substr(produced_by_work_id, 9, 1) = '-'
                AND substr(produced_by_work_id, 14, 1) = '-'
                AND substr(produced_by_work_id, 19, 1) = '-'
                AND substr(produced_by_work_id, 24, 1) = '-'
                AND substr(produced_by_work_id, 15, 1) = '7'
                AND substr(produced_by_work_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(produced_by_work_id, '-', '')) = 32
                AND replace(produced_by_work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    client_device_id TEXT NULL
        CHECK (
            client_device_id IS NULL
            OR (
                length(client_device_id) = 36
                AND substr(client_device_id, 9, 1) = '-'
                AND substr(client_device_id, 14, 1) = '-'
                AND substr(client_device_id, 19, 1) = '-'
                AND substr(client_device_id, 24, 1) = '-'
                AND substr(client_device_id, 15, 1) = '7'
                AND substr(client_device_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(client_device_id, '-', '')) = 32
                AND replace(client_device_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    client_message_id TEXT NULL
        CHECK (
            client_message_id IS NULL
            OR (
                length(client_message_id) = 36
                AND substr(client_message_id, 9, 1) = '-'
                AND substr(client_message_id, 14, 1) = '-'
                AND substr(client_message_id, 19, 1) = '-'
                AND substr(client_message_id, 24, 1) = '-'
                AND substr(client_message_id, 15, 1) = '7'
                AND substr(client_message_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(client_message_id, '-', '')) = 32
                AND replace(client_message_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    committed_at TEXT NOT NULL
        CHECK (
            length(committed_at) = 27
            AND committed_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    CHECK (
        (role = 'user' AND produced_by_work_id IS NULL AND client_device_id IS NOT NULL AND client_message_id IS NOT NULL)
        OR (role = 'assistant' AND produced_by_work_id IS NOT NULL AND client_device_id IS NULL AND client_message_id IS NULL)
        OR (role = 'system' AND produced_by_work_id IS NULL AND client_device_id IS NULL AND client_message_id IS NULL)
    ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id) REFERENCES conversations (conversation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (produced_by_work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (client_device_id) REFERENCES client_devices (device_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_messages_conversation
    ON messages (conversation_id);
CREATE UNIQUE INDEX ux_messages_client_identity
    ON messages (client_device_id, client_message_id)
    WHERE client_device_id IS NOT NULL AND client_message_id IS NOT NULL;
CREATE UNIQUE INDEX ux_messages_produced_by_work
    ON messages (produced_by_work_id)
    WHERE produced_by_work_id IS NOT NULL;

CREATE TABLE client_commands (
    device_id TEXT NOT NULL
        CHECK (
            length(device_id) = 36
            AND substr(device_id, 9, 1) = '-'
            AND substr(device_id, 14, 1) = '-'
            AND substr(device_id, 19, 1) = '-'
            AND substr(device_id, 24, 1) = '-'
            AND substr(device_id, 15, 1) = '7'
            AND substr(device_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(device_id, '-', '')) = 32
            AND replace(device_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    idempotency_key TEXT NOT NULL
        CHECK (
            length(idempotency_key) = 36
            AND substr(idempotency_key, 9, 1) = '-'
            AND substr(idempotency_key, 14, 1) = '-'
            AND substr(idempotency_key, 19, 1) = '-'
            AND substr(idempotency_key, 24, 1) = '-'
            AND substr(idempotency_key, 15, 1) = '7'
            AND substr(idempotency_key, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(idempotency_key, '-', '')) = 32
            AND replace(idempotency_key, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    command_type TEXT NOT NULL CHECK (command_type IN ('message', 'cancel')),
    request_hash TEXT NOT NULL
        CHECK (length(request_hash) = 64 AND request_hash NOT GLOB '*[^0-9a-f]*'),
    response_http_status INTEGER NOT NULL CHECK (response_http_status BETWEEN 100 AND 599),
    response_json TEXT NOT NULL CHECK (json_valid(response_json)),
    committed_cursor INTEGER NOT NULL CHECK (committed_cursor > 0),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    PRIMARY KEY (device_id, idempotency_key),
    FOREIGN KEY (device_id) REFERENCES client_devices (device_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
