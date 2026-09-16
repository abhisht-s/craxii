PRAGMA defer_foreign_keys = ON;

CREATE TEMP TABLE ch1_migration_assertion (
    valid INTEGER NOT NULL CHECK (valid = 1)
) STRICT;

INSERT INTO ch1_migration_assertion (valid)
SELECT CASE
    WHEN (SELECT COUNT(*) FROM craxii_principals) = 0
         AND (SELECT COUNT(*) FROM temp.ch1_owner_seed) = 0 THEN 1
    WHEN (SELECT COUNT(*) FROM craxii_principals) = 1
         AND (SELECT COUNT(*) FROM temp.ch1_owner_seed) = 1 THEN 1
    ELSE 0
END;

CREATE TABLE users (
    user_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(user_id) = 36
            AND substr(user_id, 9, 1) = '-'
            AND substr(user_id, 14, 1) = '-'
            AND substr(user_id, 19, 1) = '-'
            AND substr(user_id, 24, 1) = '-'
            AND substr(user_id, 15, 1) = '7'
            AND substr(user_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(user_id, '-', '')) = 32
            AND replace(user_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    craxii_id TEXT NOT NULL,
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state = 'active'),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    UNIQUE (user_id, craxii_id),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_users_craxii_id ON users (craxii_id);

INSERT INTO users (user_id, craxii_id, lifecycle_state, created_at)
SELECT seed.user_id, principal.craxii_id, 'active', principal.created_at
FROM craxii_principals AS principal
CROSS JOIN temp.ch1_owner_seed AS seed;

CREATE TABLE conversations_v6 (
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
    craxii_id TEXT NOT NULL,
    owner_user_id TEXT NOT NULL
        CHECK (
            length(owner_user_id) = 36
            AND substr(owner_user_id, 9, 1) = '-'
            AND substr(owner_user_id, 14, 1) = '-'
            AND substr(owner_user_id, 19, 1) = '-'
            AND substr(owner_user_id, 24, 1) = '-'
            AND substr(owner_user_id, 15, 1) = '7'
            AND substr(owner_user_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(owner_user_id, '-', '')) = 32
            AND replace(owner_user_id, '-', '') NOT GLOB '*[^0-9a-f]*'
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
    UNIQUE (conversation_id, owner_user_id),
    UNIQUE (conversation_id, owner_user_id, craxii_id),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (owner_user_id, craxii_id) REFERENCES users (user_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

INSERT INTO conversations_v6
    (conversation_id, craxii_id, owner_user_id, kind, lifecycle_state,
     next_work_ordinal, state_version, created_at)
SELECT conversation_id, craxii_id, seed.user_id, kind, lifecycle_state,
       next_work_ordinal, state_version, created_at
FROM conversations
CROSS JOIN temp.ch1_owner_seed AS seed;

DROP TABLE conversations;
ALTER TABLE conversations_v6 RENAME TO conversations;

CREATE UNIQUE INDEX ux_conversations_craxii_owner_kind
    ON conversations (craxii_id, owner_user_id, kind);

CREATE TABLE client_devices_v6 (
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
    user_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    token_hash TEXT NOT NULL
        CHECK (length(token_hash) = 64 AND token_hash NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 27
            AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    last_seen_at TEXT NULL
        CHECK (last_seen_at IS NULL OR (
            length(last_seen_at) = 27
            AND last_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        )),
    revoked_at TEXT NULL
        CHECK (revoked_at IS NULL OR (
            length(revoked_at) = 27
            AND revoked_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        )),
    UNIQUE (device_id, user_id),
    FOREIGN KEY (user_id) REFERENCES users (user_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

INSERT INTO client_devices_v6
    (device_id, user_id, display_name, token_hash, created_at, last_seen_at, revoked_at)
SELECT device_id, seed.user_id, display_name, token_hash, created_at, last_seen_at, revoked_at
FROM client_devices
CROSS JOIN temp.ch1_owner_seed AS seed;

DROP TABLE client_devices;
ALTER TABLE client_devices_v6 RENAME TO client_devices;

CREATE UNIQUE INDEX ux_client_devices_token_hash ON client_devices (token_hash);
CREATE INDEX ix_client_devices_user_id ON client_devices (user_id);

CREATE TABLE channel_accounts (
    channel_account_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(channel_account_id) = 36
            AND substr(channel_account_id, 9, 1) = '-'
            AND substr(channel_account_id, 14, 1) = '-'
            AND substr(channel_account_id, 19, 1) = '-'
            AND substr(channel_account_id, 24, 1) = '-'
            AND substr(channel_account_id, 15, 1) = '7'
            AND substr(channel_account_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(channel_account_id, '-', '')) = 32
            AND replace(channel_account_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    craxii_id TEXT NOT NULL,
    provider_key TEXT NOT NULL
        CHECK (
            length(CAST(provider_key AS BLOB)) BETWEEN 1 AND 64
            AND provider_key NOT GLOB '*[^a-z0-9._-]*'
        ),
    external_account_id TEXT NOT NULL
        CHECK (
            length(CAST(external_account_id AS BLOB)) BETWEEN 1 AND 255
            AND external_account_id = trim(external_account_id)
        ),
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state IN ('active', 'disabled')),
    created_at TEXT NOT NULL CHECK (
        length(created_at) = 27
        AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    disabled_at TEXT NULL CHECK (disabled_at IS NULL OR (
        length(disabled_at) = 27
        AND disabled_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    CHECK (
        (lifecycle_state = 'active' AND disabled_at IS NULL)
        OR (lifecycle_state = 'disabled' AND disabled_at IS NOT NULL AND disabled_at >= created_at)
    ),
    UNIQUE (channel_account_id, craxii_id),
    UNIQUE (craxii_id, provider_key, external_account_id),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE TABLE external_identities (
    external_identity_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(external_identity_id) = 36
            AND substr(external_identity_id, 9, 1) = '-'
            AND substr(external_identity_id, 14, 1) = '-'
            AND substr(external_identity_id, 19, 1) = '-'
            AND substr(external_identity_id, 24, 1) = '-'
            AND substr(external_identity_id, 15, 1) = '7'
            AND substr(external_identity_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(external_identity_id, '-', '')) = 32
            AND replace(external_identity_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    channel_account_id TEXT NOT NULL,
    craxii_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    external_subject_id TEXT NOT NULL
        CHECK (
            length(CAST(external_subject_id AS BLOB)) BETWEEN 1 AND 255
            AND external_subject_id = trim(external_subject_id)
        ),
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state IN ('active', 'revoked')),
    created_at TEXT NOT NULL CHECK (
        length(created_at) = 27
        AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    revoked_at TEXT NULL CHECK (revoked_at IS NULL OR (
        length(revoked_at) = 27
        AND revoked_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    CHECK (
        (lifecycle_state = 'active' AND revoked_at IS NULL)
        OR (lifecycle_state = 'revoked' AND revoked_at IS NOT NULL AND revoked_at >= created_at)
    ),
    UNIQUE (external_identity_id, channel_account_id, craxii_id, user_id),
    FOREIGN KEY (channel_account_id, craxii_id)
        REFERENCES channel_accounts (channel_account_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (user_id, craxii_id) REFERENCES users (user_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_external_identities_active_subject
    ON external_identities (channel_account_id, external_subject_id)
    WHERE lifecycle_state = 'active';

CREATE TABLE conversation_bindings (
    conversation_binding_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(conversation_binding_id) = 36
            AND substr(conversation_binding_id, 9, 1) = '-'
            AND substr(conversation_binding_id, 14, 1) = '-'
            AND substr(conversation_binding_id, 19, 1) = '-'
            AND substr(conversation_binding_id, 24, 1) = '-'
            AND substr(conversation_binding_id, 15, 1) = '7'
            AND substr(conversation_binding_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(conversation_binding_id, '-', '')) = 32
            AND replace(conversation_binding_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    channel_account_id TEXT NOT NULL,
    external_identity_id TEXT NOT NULL,
    craxii_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    external_conversation_id TEXT NOT NULL
        CHECK (
            length(CAST(external_conversation_id AS BLOB)) BETWEEN 1 AND 255
            AND external_conversation_id = trim(external_conversation_id)
        ),
    external_thread_id TEXT NULL
        CHECK (
            external_thread_id IS NULL OR (
                length(CAST(external_thread_id AS BLOB)) BETWEEN 1 AND 255
                AND external_thread_id = trim(external_thread_id)
            )
        ),
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state IN ('active', 'revoked')),
    created_at TEXT NOT NULL CHECK (
        length(created_at) = 27
        AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    revoked_at TEXT NULL CHECK (revoked_at IS NULL OR (
        length(revoked_at) = 27
        AND revoked_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    CHECK (
        (lifecycle_state = 'active' AND revoked_at IS NULL)
        OR (lifecycle_state = 'revoked' AND revoked_at IS NOT NULL AND revoked_at >= created_at)
    ),
    UNIQUE (conversation_binding_id, conversation_id),
    UNIQUE (conversation_binding_id, channel_account_id, external_identity_id, craxii_id, user_id, conversation_id),
    FOREIGN KEY (channel_account_id, craxii_id)
        REFERENCES channel_accounts (channel_account_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (external_identity_id, channel_account_id, craxii_id, user_id)
        REFERENCES external_identities (external_identity_id, channel_account_id, craxii_id, user_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, user_id, craxii_id)
        REFERENCES conversations (conversation_id, owner_user_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_conversation_bindings_active_destination
    ON conversation_bindings (
        channel_account_id,
        external_conversation_id,
        COALESCE(external_thread_id, '')
    ) WHERE lifecycle_state = 'active';

CREATE TABLE inbound_deliveries (
    inbound_delivery_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(inbound_delivery_id) = 36
            AND substr(inbound_delivery_id, 9, 1) = '-'
            AND substr(inbound_delivery_id, 14, 1) = '-'
            AND substr(inbound_delivery_id, 19, 1) = '-'
            AND substr(inbound_delivery_id, 24, 1) = '-'
            AND substr(inbound_delivery_id, 15, 1) = '7'
            AND substr(inbound_delivery_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(inbound_delivery_id, '-', '')) = 32
            AND replace(inbound_delivery_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    channel_account_id TEXT NOT NULL,
    craxii_id TEXT NOT NULL,
    external_event_id TEXT NOT NULL
        CHECK (length(CAST(external_event_id AS BLOB)) BETWEEN 1 AND 255 AND external_event_id = trim(external_event_id)),
    external_message_id TEXT NULL
        CHECK (external_message_id IS NULL OR (length(CAST(external_message_id AS BLOB)) BETWEEN 1 AND 255 AND external_message_id = trim(external_message_id))),
    external_subject_id TEXT NOT NULL
        CHECK (length(CAST(external_subject_id AS BLOB)) BETWEEN 1 AND 255 AND external_subject_id = trim(external_subject_id)),
    external_conversation_id TEXT NOT NULL
        CHECK (length(CAST(external_conversation_id AS BLOB)) BETWEEN 1 AND 255 AND external_conversation_id = trim(external_conversation_id)),
    external_thread_id TEXT NULL
        CHECK (external_thread_id IS NULL OR (length(CAST(external_thread_id AS BLOB)) BETWEEN 1 AND 255 AND external_thread_id = trim(external_thread_id))),
    material_sha256 TEXT NOT NULL
        CHECK (length(material_sha256) = 64 AND material_sha256 NOT GLOB '*[^0-9a-f]*'),
    provider_occurred_at TEXT NULL CHECK (provider_occurred_at IS NULL OR (
        length(provider_occurred_at) = 27
        AND provider_occurred_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    received_at TEXT NOT NULL CHECK (
        length(received_at) = 27
        AND received_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    ),
    receipt_state TEXT NOT NULL CHECK (receipt_state IN ('received', 'classified')),
    classification TEXT NULL CHECK (classification IS NULL OR classification IN ('message', 'control', 'rejected', 'unsupported')),
    external_identity_id TEXT NULL,
    conversation_binding_id TEXT NULL,
    user_id TEXT NULL,
    conversation_id TEXT NULL,
    message_id TEXT NULL,
    work_id TEXT NULL,
    control_target_work_id TEXT NULL,
    classified_at TEXT NULL CHECK (classified_at IS NULL OR (
        length(classified_at) = 27
        AND classified_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
    )),
    CHECK (
        (receipt_state = 'received'
            AND classification IS NULL
            AND external_identity_id IS NULL
            AND conversation_binding_id IS NULL
            AND user_id IS NULL
            AND conversation_id IS NULL
            AND message_id IS NULL
            AND work_id IS NULL
            AND control_target_work_id IS NULL
            AND classified_at IS NULL)
        OR
        (receipt_state = 'classified' AND classified_at IS NOT NULL AND (
            (classification = 'message'
                AND external_message_id IS NOT NULL
                AND external_identity_id IS NOT NULL
                AND conversation_binding_id IS NOT NULL
                AND user_id IS NOT NULL
                AND conversation_id IS NOT NULL
                AND message_id IS NOT NULL
                AND work_id IS NOT NULL
                AND control_target_work_id IS NULL)
            OR
            (classification = 'control'
                AND external_identity_id IS NOT NULL
                AND conversation_binding_id IS NOT NULL
                AND user_id IS NOT NULL
                AND conversation_id IS NOT NULL
                AND message_id IS NULL
                AND work_id IS NULL)
            OR
            (classification IN ('rejected', 'unsupported')
                AND external_identity_id IS NULL
                AND conversation_binding_id IS NULL
                AND user_id IS NULL
                AND conversation_id IS NULL
                AND message_id IS NULL
                AND work_id IS NULL
                AND control_target_work_id IS NULL)
        ))
    ),
    UNIQUE (inbound_delivery_id, channel_account_id, craxii_id),
    UNIQUE (inbound_delivery_id, user_id),
    FOREIGN KEY (channel_account_id, craxii_id)
        REFERENCES channel_accounts (channel_account_id, craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (external_identity_id, channel_account_id, craxii_id, user_id)
        REFERENCES external_identities (external_identity_id, channel_account_id, craxii_id, user_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_binding_id, channel_account_id, external_identity_id, craxii_id, user_id, conversation_id)
        REFERENCES conversation_bindings (conversation_binding_id, channel_account_id, external_identity_id, craxii_id, user_id, conversation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (message_id) REFERENCES messages (message_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (control_target_work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_inbound_deliveries_account_event
    ON inbound_deliveries (channel_account_id, external_event_id);
CREATE UNIQUE INDEX ux_inbound_deliveries_account_message
    ON inbound_deliveries (
        channel_account_id,
        external_conversation_id,
        COALESCE(external_thread_id, ''),
        external_message_id
    ) WHERE external_message_id IS NOT NULL;

ALTER TABLE work_items ADD COLUMN reply_binding_id TEXT NULL
    CHECK (
        reply_binding_id IS NULL OR (
            length(reply_binding_id) = 36
            AND substr(reply_binding_id, 9, 1) = '-'
            AND substr(reply_binding_id, 14, 1) = '-'
            AND substr(reply_binding_id, 19, 1) = '-'
            AND substr(reply_binding_id, 24, 1) = '-'
            AND substr(reply_binding_id, 15, 1) = '7'
            AND substr(reply_binding_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(reply_binding_id, '-', '')) = 32
            AND replace(reply_binding_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        )
    ) REFERENCES conversation_bindings (conversation_binding_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT;

CREATE TABLE messages_v6 (
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
    author_user_id TEXT NULL
        CHECK (author_user_id IS NULL OR (
            length(author_user_id) = 36
            AND substr(author_user_id, 9, 1) = '-'
            AND substr(author_user_id, 14, 1) = '-'
            AND substr(author_user_id, 19, 1) = '-'
            AND substr(author_user_id, 24, 1) = '-'
            AND substr(author_user_id, 15, 1) = '7'
            AND substr(author_user_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(author_user_id, '-', '')) = 32
            AND replace(author_user_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        )),
    produced_by_work_id TEXT NULL
        CHECK (produced_by_work_id IS NULL OR (
            length(produced_by_work_id) = 36
            AND substr(produced_by_work_id, 9, 1) = '-'
            AND substr(produced_by_work_id, 14, 1) = '-'
            AND substr(produced_by_work_id, 19, 1) = '-'
            AND substr(produced_by_work_id, 24, 1) = '-'
            AND substr(produced_by_work_id, 15, 1) = '7'
            AND substr(produced_by_work_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(produced_by_work_id, '-', '')) = 32
            AND replace(produced_by_work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        )),
    client_device_id TEXT NULL
        CHECK (client_device_id IS NULL OR (
            length(client_device_id) = 36
            AND substr(client_device_id, 9, 1) = '-'
            AND substr(client_device_id, 14, 1) = '-'
            AND substr(client_device_id, 19, 1) = '-'
            AND substr(client_device_id, 24, 1) = '-'
            AND substr(client_device_id, 15, 1) = '7'
            AND substr(client_device_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(client_device_id, '-', '')) = 32
            AND replace(client_device_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        )),
    client_message_id TEXT NULL
        CHECK (client_message_id IS NULL OR (
            length(client_message_id) = 36
            AND substr(client_message_id, 9, 1) = '-'
            AND substr(client_message_id, 14, 1) = '-'
            AND substr(client_message_id, 19, 1) = '-'
            AND substr(client_message_id, 24, 1) = '-'
            AND substr(client_message_id, 15, 1) = '7'
            AND substr(client_message_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(client_message_id, '-', '')) = 32
            AND replace(client_message_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        )),
    inbound_delivery_id TEXT NULL
        CHECK (inbound_delivery_id IS NULL OR (
            length(inbound_delivery_id) = 36
            AND substr(inbound_delivery_id, 9, 1) = '-'
            AND substr(inbound_delivery_id, 14, 1) = '-'
            AND substr(inbound_delivery_id, 19, 1) = '-'
            AND substr(inbound_delivery_id, 24, 1) = '-'
            AND substr(inbound_delivery_id, 15, 1) = '7'
            AND substr(inbound_delivery_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(inbound_delivery_id, '-', '')) = 32
            AND replace(inbound_delivery_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        )),
    committed_at TEXT NOT NULL
        CHECK (
            length(committed_at) = 27
            AND committed_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    CHECK (
        (role = 'user'
            AND author_user_id IS NOT NULL
            AND produced_by_work_id IS NULL
            AND client_device_id IS NOT NULL
            AND client_message_id IS NOT NULL
            AND inbound_delivery_id IS NULL)
        OR (role = 'user'
            AND author_user_id IS NOT NULL
            AND produced_by_work_id IS NULL
            AND client_device_id IS NULL
            AND client_message_id IS NULL
            AND inbound_delivery_id IS NOT NULL)
        OR (role = 'assistant'
            AND author_user_id IS NULL
            AND produced_by_work_id IS NOT NULL
            AND client_device_id IS NULL
            AND client_message_id IS NULL
            AND inbound_delivery_id IS NULL)
        OR (role = 'system'
            AND author_user_id IS NULL
            AND produced_by_work_id IS NULL
            AND client_device_id IS NULL
            AND client_message_id IS NULL
            AND inbound_delivery_id IS NULL)
    ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id) REFERENCES conversations (conversation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, author_user_id)
        REFERENCES conversations (conversation_id, owner_user_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (produced_by_work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (client_device_id, author_user_id)
        REFERENCES client_devices (device_id, user_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (inbound_delivery_id, author_user_id)
        REFERENCES inbound_deliveries (inbound_delivery_id, user_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;

INSERT INTO messages_v6
    (message_id, craxii_id, conversation_id, role, content_json, content_sha256,
     author_user_id, produced_by_work_id, client_device_id, client_message_id,
     inbound_delivery_id, committed_at)
SELECT message_id, craxii_id, conversation_id, role, content_json, content_sha256,
       CASE WHEN role = 'user' THEN seed.user_id ELSE NULL END,
       produced_by_work_id, client_device_id, client_message_id, NULL, committed_at
FROM messages
LEFT JOIN temp.ch1_owner_seed AS seed ON role = 'user';

DROP TABLE messages;
ALTER TABLE messages_v6 RENAME TO messages;

CREATE INDEX ix_messages_conversation ON messages (conversation_id);
CREATE INDEX ix_messages_author_user_id ON messages (author_user_id)
    WHERE author_user_id IS NOT NULL;
CREATE UNIQUE INDEX ux_messages_client_identity
    ON messages (client_device_id, client_message_id)
    WHERE client_device_id IS NOT NULL AND client_message_id IS NOT NULL;
CREATE UNIQUE INDEX ux_messages_produced_by_work
    ON messages (produced_by_work_id)
    WHERE produced_by_work_id IS NOT NULL;
CREATE UNIQUE INDEX ux_messages_inbound_delivery
    ON messages (inbound_delivery_id)
    WHERE inbound_delivery_id IS NOT NULL;

DELETE FROM ch1_migration_assertion;
INSERT INTO ch1_migration_assertion (valid)
SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check) THEN 1 ELSE 0 END;

DROP TABLE ch1_migration_assertion;
DROP TABLE temp.ch1_owner_seed;
