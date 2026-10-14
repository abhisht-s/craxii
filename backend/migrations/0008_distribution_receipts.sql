-- Forward distribution provenance and transfer evidence. No duplicate task records.
CREATE TABLE distribution_owners (
 environment TEXT NOT NULL CHECK(environment='development'), customer_id TEXT NOT NULL, logical_agent_id TEXT NOT NULL,
 user_id TEXT NOT NULL UNIQUE, craxii_id TEXT NOT NULL, highest_generation INTEGER NOT NULL CHECK(highest_generation>0), current_installation_id TEXT NOT NULL,
 PRIMARY KEY(environment,customer_id,logical_agent_id), UNIQUE(environment,customer_id,logical_agent_id,user_id,craxii_id),
 FOREIGN KEY(user_id,craxii_id) REFERENCES users(user_id,craxii_id) ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE TABLE distribution_conversations (
 environment TEXT NOT NULL, customer_id TEXT NOT NULL, logical_agent_id TEXT NOT NULL, central_conversation_id TEXT NOT NULL,
 user_id TEXT NOT NULL, craxii_id TEXT NOT NULL, local_conversation_id TEXT NOT NULL UNIQUE,
 next_sequence INTEGER NOT NULL DEFAULT 1 CHECK(next_sequence>0),
 PRIMARY KEY(environment,central_conversation_id), UNIQUE(environment,customer_id,logical_agent_id,central_conversation_id),
 FOREIGN KEY(environment,customer_id,logical_agent_id,user_id,craxii_id) REFERENCES distribution_owners(environment,customer_id,logical_agent_id,user_id,craxii_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
 FOREIGN KEY(local_conversation_id,user_id) REFERENCES conversations(conversation_id,owner_user_id) ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE TABLE distribution_receipts (
 environment TEXT NOT NULL, central_work_id TEXT NOT NULL CHECK(length(central_work_id)=36 AND substr(central_work_id,15,1)='4'), customer_id TEXT NOT NULL, logical_agent_id TEXT NOT NULL,
 central_conversation_id TEXT NOT NULL, central_sequence INTEGER NOT NULL CHECK(central_sequence>0),
 material_digest TEXT NOT NULL CHECK(length(material_digest)=64 AND material_digest NOT GLOB '*[^0-9a-f]*'), material_canonical TEXT NOT NULL CHECK(length(CAST(material_canonical AS BLOB))<=524288 AND json_valid(material_canonical)),
 receipt_id TEXT NOT NULL UNIQUE CHECK(length(receipt_id)=36 AND substr(receipt_id,15,1)='7'), receipt_canonical TEXT NOT NULL CHECK(length(CAST(receipt_canonical AS BLOB))<=16384 AND json_valid(receipt_canonical)),
 user_id TEXT NOT NULL, local_conversation_id TEXT NOT NULL, local_message_id TEXT NOT NULL UNIQUE, local_work_id TEXT NOT NULL UNIQUE,
 acceptance_event_id TEXT NOT NULL UNIQUE, journal_offset INTEGER NOT NULL CHECK(journal_offset>0), local_work_ordinal INTEGER NOT NULL CHECK(local_work_ordinal>0),
 origin_installation_id TEXT NOT NULL, origin_generation INTEGER NOT NULL CHECK(origin_generation>0), admitted_at TEXT NOT NULL,
 quarantined INTEGER NOT NULL DEFAULT 0 CHECK(quarantined IN (0,1)),
 central_acknowledged INTEGER NOT NULL DEFAULT 0 CHECK(central_acknowledged IN (0,1)),
 PRIMARY KEY(environment,central_work_id), UNIQUE(receipt_id,user_id,local_message_id),
 FOREIGN KEY(environment,customer_id,logical_agent_id,central_conversation_id) REFERENCES distribution_conversations(environment,customer_id,logical_agent_id,central_conversation_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
 FOREIGN KEY(local_conversation_id,user_id) REFERENCES conversations(conversation_id,owner_user_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
 FOREIGN KEY(local_message_id,user_id,receipt_id) REFERENCES messages(message_id,author_user_id,distribution_receipt_id) ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(local_work_id) REFERENCES work_items(work_id) ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(acceptance_event_id) REFERENCES journal_events(event_id) ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE INDEX ix_distribution_receipts_pending ON distribution_receipts(central_acknowledged,central_sequence);
CREATE TABLE distribution_controls (
 environment TEXT NOT NULL, control_id TEXT NOT NULL CHECK(length(control_id)=36 AND substr(control_id,15,1)='4'), customer_id TEXT NOT NULL, logical_agent_id TEXT NOT NULL,
 central_work_id TEXT NOT NULL, material_digest TEXT NOT NULL CHECK(length(material_digest)=64 AND material_digest NOT GLOB '*[^0-9a-f]*'),
 control_digest TEXT NOT NULL CHECK(length(control_digest)=64 AND control_digest NOT GLOB '*[^0-9a-f]*'), control_canonical TEXT NOT NULL CHECK(json_valid(control_canonical) AND length(CAST(control_canonical AS BLOB))<=16384),
 receipt_canonical TEXT NOT NULL CHECK(json_valid(receipt_canonical) AND length(CAST(receipt_canonical AS BLOB))<=16384),
 central_conversation_id TEXT NULL, central_sequence INTEGER NULL CHECK(central_sequence IS NULL OR central_sequence>0), target_material_canonical TEXT NULL CHECK(target_material_canonical IS NULL OR (json_valid(target_material_canonical) AND length(CAST(target_material_canonical AS BLOB))<=524288)),
 local_work_id TEXT NULL, cancellation_event_id TEXT NULL, sequence_reconciled INTEGER NOT NULL DEFAULT 0 CHECK(sequence_reconciled IN (0,1)),
 quarantined INTEGER NOT NULL DEFAULT 0 CHECK(quarantined IN (0,1)),
 central_acknowledged INTEGER NOT NULL DEFAULT 0 CHECK(central_acknowledged IN (0,1)),
 PRIMARY KEY(environment,control_id), FOREIGN KEY(environment,customer_id,logical_agent_id) REFERENCES distribution_owners(environment,customer_id,logical_agent_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
 FOREIGN KEY(local_work_id) REFERENCES work_items(work_id) ON UPDATE RESTRICT ON DELETE RESTRICT, FOREIGN KEY(cancellation_event_id) REFERENCES journal_events(event_id) ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX ix_distribution_controls_target ON distribution_controls(environment,central_work_id);
CREATE INDEX ix_distribution_controls_pending ON distribution_controls(central_acknowledged,control_id);
CREATE TABLE messages_v8 (
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
    distribution_receipt_id TEXT NULL CHECK(distribution_receipt_id IS NULL OR (length(distribution_receipt_id)=36 AND substr(distribution_receipt_id,15,1)='7')),
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
            AND inbound_delivery_id IS NULL AND distribution_receipt_id IS NULL)
        OR (role = 'user'
            AND author_user_id IS NOT NULL
            AND produced_by_work_id IS NULL
            AND client_device_id IS NULL
            AND client_message_id IS NULL
            AND inbound_delivery_id IS NOT NULL AND distribution_receipt_id IS NULL)
        OR (role = 'user' AND author_user_id IS NOT NULL AND produced_by_work_id IS NULL AND client_device_id IS NULL AND client_message_id IS NULL AND inbound_delivery_id IS NULL AND distribution_receipt_id IS NOT NULL)
        OR (role = 'assistant'
            AND author_user_id IS NULL
            AND produced_by_work_id IS NOT NULL
            AND client_device_id IS NULL
            AND client_message_id IS NULL
            AND inbound_delivery_id IS NULL AND distribution_receipt_id IS NULL)
        OR (role = 'system'
            AND author_user_id IS NULL
            AND produced_by_work_id IS NULL
            AND client_device_id IS NULL
            AND client_message_id IS NULL
            AND inbound_delivery_id IS NULL AND distribution_receipt_id IS NULL)
    ),
    UNIQUE(message_id,author_user_id,distribution_receipt_id),
    FOREIGN KEY(distribution_receipt_id,author_user_id,message_id) REFERENCES distribution_receipts(receipt_id,user_id,local_message_id) ON UPDATE RESTRICT ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
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

INSERT INTO messages_v8(message_id,craxii_id,conversation_id,role,content_json,content_sha256,author_user_id,produced_by_work_id,client_device_id,client_message_id,inbound_delivery_id,committed_at) SELECT message_id,craxii_id,conversation_id,role,content_json,content_sha256,author_user_id,produced_by_work_id,client_device_id,client_message_id,inbound_delivery_id,committed_at FROM messages;
DROP TABLE messages;
ALTER TABLE messages_v8 RENAME TO messages;
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
CREATE UNIQUE INDEX ux_messages_distribution_receipt ON messages(distribution_receipt_id) WHERE distribution_receipt_id IS NOT NULL;
