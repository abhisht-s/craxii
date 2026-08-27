CREATE TABLE journal_events (
    journal_offset INTEGER PRIMARY KEY AUTOINCREMENT
        CHECK (journal_offset > 0),
    event_id TEXT NOT NULL
        CHECK (
            length(event_id) = 36
            AND substr(event_id, 9, 1) = '-'
            AND substr(event_id, 14, 1) = '-'
            AND substr(event_id, 19, 1) = '-'
            AND substr(event_id, 24, 1) = '-'
            AND substr(event_id, 15, 1) = '7'
            AND substr(event_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(event_id, '-', '')) = 32
            AND replace(event_id, '-', '') NOT GLOB '*[^0-9a-f]*'
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
    stream_id TEXT NOT NULL
        CHECK (
            (
                substr(stream_id, 1, 7) = 'craxii:'
                AND length(stream_id) = 43
                AND substr(stream_id, 16, 1) = '-'
                AND substr(stream_id, 21, 1) = '-'
                AND substr(stream_id, 26, 1) = '-'
                AND substr(stream_id, 31, 1) = '-'
                AND substr(stream_id, 22, 1) = '7'
                AND substr(stream_id, 27, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 8), '-', '')) = 32
                AND replace(substr(stream_id, 8), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
            OR (
                substr(stream_id, 1, 13) = 'conversation:'
                AND length(stream_id) = 49
                AND substr(stream_id, 22, 1) = '-'
                AND substr(stream_id, 27, 1) = '-'
                AND substr(stream_id, 32, 1) = '-'
                AND substr(stream_id, 37, 1) = '-'
                AND substr(stream_id, 28, 1) = '7'
                AND substr(stream_id, 33, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 14), '-', '')) = 32
                AND replace(substr(stream_id, 14), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
            OR (
                substr(stream_id, 1, 5) = 'work:'
                AND length(stream_id) = 41
                AND substr(stream_id, 14, 1) = '-'
                AND substr(stream_id, 19, 1) = '-'
                AND substr(stream_id, 24, 1) = '-'
                AND substr(stream_id, 29, 1) = '-'
                AND substr(stream_id, 20, 1) = '7'
                AND substr(stream_id, 25, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 6), '-', '')) = 32
                AND replace(substr(stream_id, 6), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
            OR (
                substr(stream_id, 1, 8) = 'runtime:'
                AND length(stream_id) = 44
                AND substr(stream_id, 17, 1) = '-'
                AND substr(stream_id, 22, 1) = '-'
                AND substr(stream_id, 27, 1) = '-'
                AND substr(stream_id, 32, 1) = '-'
                AND substr(stream_id, 23, 1) = '7'
                AND substr(stream_id, 28, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 9), '-', '')) = 32
                AND replace(substr(stream_id, 9), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    stream_seq INTEGER NOT NULL CHECK (stream_seq > 0),
    event_type TEXT NOT NULL
        CHECK (
            length(event_type) BETWEEN 3 AND 128
            AND event_type NOT GLOB '*[^a-z0-9_.]*'
            AND substr(event_type, 1, 1) BETWEEN 'a' AND 'z'
            AND substr(event_type, -1, 1) BETWEEN 'a' AND 'z'
            AND instr(event_type, '.') > 1
            AND event_type NOT GLOB '*..*'
        ),
    event_version INTEGER NOT NULL CHECK (event_version > 0),
    conversation_id TEXT NULL
        CHECK (
            conversation_id IS NULL
            OR (
                length(conversation_id) = 36
                AND substr(conversation_id, 9, 1) = '-'
                AND substr(conversation_id, 14, 1) = '-'
                AND substr(conversation_id, 19, 1) = '-'
                AND substr(conversation_id, 24, 1) = '-'
                AND substr(conversation_id, 15, 1) = '7'
                AND substr(conversation_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(conversation_id, '-', '')) = 32
                AND replace(conversation_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    work_id TEXT NULL
        CHECK (
            work_id IS NULL
            OR (
                length(work_id) = 36
                AND substr(work_id, 9, 1) = '-'
                AND substr(work_id, 14, 1) = '-'
                AND substr(work_id, 19, 1) = '-'
                AND substr(work_id, 24, 1) = '-'
                AND substr(work_id, 15, 1) = '7'
                AND substr(work_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(work_id, '-', '')) = 32
                AND replace(work_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    causation_event_id TEXT NULL
        CHECK (
            causation_event_id IS NULL
            OR (
                length(causation_event_id) = 36
                AND substr(causation_event_id, 9, 1) = '-'
                AND substr(causation_event_id, 14, 1) = '-'
                AND substr(causation_event_id, 19, 1) = '-'
                AND substr(causation_event_id, 24, 1) = '-'
                AND substr(causation_event_id, 15, 1) = '7'
                AND substr(causation_event_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(causation_event_id, '-', '')) = 32
                AND replace(causation_event_id, '-', '') NOT GLOB '*[^0-9a-f]*'
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
    actor_kind TEXT NOT NULL
        CHECK (actor_kind IN ('user', 'craxii', 'model', 'tool', 'runtime', 'client')),
    actor_id TEXT NULL
        CHECK (
            actor_id IS NULL
            OR (
                length(actor_id) = 36
                AND substr(actor_id, 9, 1) = '-'
                AND substr(actor_id, 14, 1) = '-'
                AND substr(actor_id, 19, 1) = '-'
                AND substr(actor_id, 24, 1) = '-'
                AND substr(actor_id, 15, 1) = '7'
                AND substr(actor_id, 20, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(actor_id, '-', '')) = 32
                AND replace(actor_id, '-', '') NOT GLOB '*[^0-9a-f]*'
            )
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
    payload_json TEXT NOT NULL
        CHECK (
            json_valid(payload_json)
            AND json_type(payload_json) = 'object'
            AND length(CAST(payload_json AS BLOB)) <= 262144
        ),
    payload_sha256 TEXT NOT NULL
        CHECK (
            length(payload_sha256) = 64
            AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
        ),
    recorded_at TEXT NOT NULL
        CHECK (
            length(recorded_at) = 27
            AND recorded_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    occurred_at TEXT NULL
        CHECK (
            occurred_at IS NULL
            OR (
                length(occurred_at) = 27
                AND occurred_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
            )
        ),
    FOREIGN KEY (craxii_id) REFERENCES craxii_principals (craxii_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id) REFERENCES conversations (conversation_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (causation_event_id) REFERENCES journal_events (event_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (runtime_instance_id) REFERENCES runtime_instances (runtime_instance_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_journal_events_event_id
    ON journal_events (event_id);
CREATE UNIQUE INDEX ux_journal_events_stream_sequence
    ON journal_events (stream_id, stream_seq);
CREATE INDEX ix_journal_events_conversation_offset
    ON journal_events (conversation_id, journal_offset)
    WHERE conversation_id IS NOT NULL;
CREATE INDEX ix_journal_events_work_offset
    ON journal_events (work_id, journal_offset)
    WHERE work_id IS NOT NULL;

CREATE TABLE stream_heads (
    stream_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            (
                substr(stream_id, 1, 7) = 'craxii:'
                AND length(stream_id) = 43
                AND substr(stream_id, 16, 1) = '-'
                AND substr(stream_id, 21, 1) = '-'
                AND substr(stream_id, 26, 1) = '-'
                AND substr(stream_id, 31, 1) = '-'
                AND substr(stream_id, 22, 1) = '7'
                AND substr(stream_id, 27, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 8), '-', '')) = 32
                AND replace(substr(stream_id, 8), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
            OR (
                substr(stream_id, 1, 13) = 'conversation:'
                AND length(stream_id) = 49
                AND substr(stream_id, 22, 1) = '-'
                AND substr(stream_id, 27, 1) = '-'
                AND substr(stream_id, 32, 1) = '-'
                AND substr(stream_id, 37, 1) = '-'
                AND substr(stream_id, 28, 1) = '7'
                AND substr(stream_id, 33, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 14), '-', '')) = 32
                AND replace(substr(stream_id, 14), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
            OR (
                substr(stream_id, 1, 5) = 'work:'
                AND length(stream_id) = 41
                AND substr(stream_id, 14, 1) = '-'
                AND substr(stream_id, 19, 1) = '-'
                AND substr(stream_id, 24, 1) = '-'
                AND substr(stream_id, 29, 1) = '-'
                AND substr(stream_id, 20, 1) = '7'
                AND substr(stream_id, 25, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 6), '-', '')) = 32
                AND replace(substr(stream_id, 6), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
            OR (
                substr(stream_id, 1, 8) = 'runtime:'
                AND length(stream_id) = 44
                AND substr(stream_id, 17, 1) = '-'
                AND substr(stream_id, 22, 1) = '-'
                AND substr(stream_id, 27, 1) = '-'
                AND substr(stream_id, 32, 1) = '-'
                AND substr(stream_id, 23, 1) = '7'
                AND substr(stream_id, 28, 1) IN ('8', '9', 'a', 'b')
                AND length(replace(substr(stream_id, 9), '-', '')) = 32
                AND replace(substr(stream_id, 9), '-', '') NOT GLOB '*[^0-9a-f]*'
            )
        ),
    last_stream_seq INTEGER NOT NULL CHECK (last_stream_seq >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE work_item_inputs (
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
    input_event_id TEXT NOT NULL
        CHECK (
            length(input_event_id) = 36
            AND substr(input_event_id, 9, 1) = '-'
            AND substr(input_event_id, 14, 1) = '-'
            AND substr(input_event_id, 19, 1) = '-'
            AND substr(input_event_id, 24, 1) = '-'
            AND substr(input_event_id, 15, 1) = '7'
            AND substr(input_event_id, 20, 1) IN ('8', '9', 'a', 'b')
            AND length(replace(input_event_id, '-', '')) = 32
            AND replace(input_event_id, '-', '') NOT GLOB '*[^0-9a-f]*'
        ),
    relationship TEXT NOT NULL
        CHECK (
            relationship IN (
                'trigger',
                'steering',
                'supplemental',
                'scheduled_trigger',
                'external_trigger',
                'recovery_instruction'
            )
        ),
    ordinal_within_work INTEGER NOT NULL CHECK (ordinal_within_work > 0),
    attached_at TEXT NOT NULL
        CHECK (
            length(attached_at) = 27
            AND attached_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9]Z'
        ),
    attached_by_actor TEXT NOT NULL
        CHECK (attached_by_actor IN ('user', 'craxii', 'system', 'recovery')),
    PRIMARY KEY (work_id, input_event_id),
    FOREIGN KEY (work_id) REFERENCES work_items (work_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (input_event_id) REFERENCES journal_events (event_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ux_work_item_inputs_work_ordinal
    ON work_item_inputs (work_id, ordinal_within_work);
