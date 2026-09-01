ALTER TABLE model_invocations ADD COLUMN usage_status TEXT NOT NULL DEFAULT 'not_applicable_or_unknown'
    CHECK (usage_status IN ('reported', 'unavailable', 'not_applicable_or_unknown'));

ALTER TABLE model_invocations ADD COLUMN provider_reported_usage_json TEXT NULL
    CHECK (
        provider_reported_usage_json IS NULL
        OR (
            length(provider_reported_usage_json) <= 4096
            AND json_valid(provider_reported_usage_json)
            AND json_type(provider_reported_usage_json) = 'object'
        )
    );

ALTER TABLE model_invocations ADD COLUMN provider_error_kind TEXT NULL
    CHECK (
        provider_error_kind IS NULL
        OR provider_error_kind IN (
            'rate_limited',
            'temporarily_unavailable',
            'transport_before_response',
            'timeout_before_output',
            'authentication',
            'authorization',
            'invalid_request',
            'unknown_model',
            'transport_after_possible_processing',
            'timeout_after_output',
            'malformed_response',
            'malformed_completed_tool_arguments',
            'output_too_large',
            'unsupported_response_item',
            'context_error',
            'safety_refusal',
            'cancelled',
            'provider_outcome_unknown',
            'internal_provider_error',
            'script_mismatch',
            'invalid_scripted_provider_program'
        )
    );

ALTER TABLE model_invocations ADD COLUMN provider_outcome_certainty TEXT NULL
    CHECK (
        provider_outcome_certainty IS NULL
        OR provider_outcome_certainty IN (
            'definitely_not_sent',
            'definite_provider_failure',
            'definitely_completed',
            'semantic_output_observed',
            'outcome_unknown'
        )
    );

ALTER TABLE model_invocations ADD COLUMN retry_reason TEXT NULL
    CHECK (
        retry_reason IS NULL
        OR retry_reason IN (
            'classified_transient_before_output',
            'semantic_output_observed',
            'nonretryable_category',
            'provider_outcome_ambiguous',
            'attempt_cap_reached',
            'cancelled',
            'deadline_exhausted'
        )
    );

ALTER TABLE model_invocations ADD COLUMN retry_delay_ms INTEGER NULL
    CHECK (retry_delay_ms IS NULL OR retry_delay_ms BETWEEN 0 AND 30000);

ALTER TABLE model_invocations ADD COLUMN provider_retry_after_ms INTEGER NULL
    CHECK (provider_retry_after_ms IS NULL OR provider_retry_after_ms BETWEEN 0 AND 30000);

ALTER TABLE model_invocations ADD COLUMN billing_ambiguity INTEGER NOT NULL DEFAULT 0
    CHECK (billing_ambiguity IN (0, 1));

UPDATE model_invocations
SET usage_status = CASE
        WHEN input_tokens IS NOT NULL THEN 'reported'
        WHEN state IN ('completed', 'failed', 'cancelled_locally', 'provider_outcome_unknown')
            THEN 'unavailable'
        ELSE 'not_applicable_or_unknown'
    END,
    provider_error_kind = CASE state
        WHEN 'failed' THEN 'internal_provider_error'
        WHEN 'cancelled_locally' THEN 'cancelled'
        WHEN 'provider_outcome_unknown' THEN 'provider_outcome_unknown'
        ELSE NULL
    END,
    provider_outcome_certainty = CASE state
        WHEN 'completed' THEN 'definitely_completed'
        WHEN 'failed' THEN 'definite_provider_failure'
        WHEN 'cancelled_locally' THEN 'outcome_unknown'
        WHEN 'provider_outcome_unknown' THEN 'outcome_unknown'
        ELSE NULL
    END,
    billing_ambiguity = CASE
        WHEN state IN ('cancelled_locally', 'provider_outcome_unknown') THEN 1
        ELSE 0
    END;

-- The final additive column carries the cross-column V4 contract. SQLite evaluates this CHECK on
-- every subsequent insert/update, while the preceding backfill lets existing immutable V3 facts
-- satisfy it without rebuilding the table or its foreign-key dependants.
ALTER TABLE model_invocations ADD COLUMN attempt_evidence_version INTEGER NOT NULL DEFAULT 1
CHECK (
    attempt_evidence_version = 1
    AND (
        (
            usage_status = 'reported'
            AND (
                (
                    state = 'completed'
                    AND input_tokens IS NOT NULL
                    AND cached_input_tokens IS NOT NULL
                    AND output_tokens IS NOT NULL
                    AND reasoning_tokens IS NOT NULL
                    AND total_tokens IS NOT NULL
                    AND provider_reported_usage_json IS NULL
                )
                OR (
                    state IN ('failed', 'cancelled_locally', 'provider_outcome_unknown')
                    AND provider_reported_usage_json IS NOT NULL
                )
            )
        )
        OR (
            usage_status = 'unavailable'
            AND state IN ('completed', 'failed', 'cancelled_locally', 'provider_outcome_unknown')
            AND input_tokens IS NULL
            AND cached_input_tokens IS NULL
            AND output_tokens IS NULL
            AND reasoning_tokens IS NULL
            AND total_tokens IS NULL
            AND provider_reported_usage_json IS NULL
        )
        OR (
            usage_status = 'not_applicable_or_unknown'
            AND state IN ('requesting', 'streaming')
            AND input_tokens IS NULL
            AND cached_input_tokens IS NULL
            AND output_tokens IS NULL
            AND reasoning_tokens IS NULL
            AND total_tokens IS NULL
            AND provider_reported_usage_json IS NULL
        )
    )
    AND (
        (
            attempt_no = 1
            AND retry_of_invocation_id IS NULL
            AND retry_reason IS NULL
            AND retry_delay_ms IS NULL
            AND provider_retry_after_ms IS NULL
        )
        OR (
            attempt_no > 1
            AND retry_of_invocation_id IS NOT NULL
            AND (
                (
                    retry_reason IS NULL
                    AND retry_delay_ms IS NULL
                    AND provider_retry_after_ms IS NULL
                )
                OR (
                    retry_reason = 'classified_transient_before_output'
                    AND retry_delay_ms IS NOT NULL
                )
            )
        )
    )
    AND (
        (
            state IN ('requesting', 'streaming')
            AND provider_error_kind IS NULL
            AND provider_outcome_certainty IS NULL
            AND billing_ambiguity = 0
        )
        OR (
            state = 'completed'
            AND provider_error_kind IS NULL
            AND provider_outcome_certainty = 'definitely_completed'
            AND billing_ambiguity = 0
        )
        OR (
            state = 'failed'
            AND provider_error_kind IS NOT NULL
            AND provider_outcome_certainty IN ('definitely_not_sent', 'definite_provider_failure', 'semantic_output_observed')
            AND billing_ambiguity = 0
        )
        OR (
            state = 'cancelled_locally'
            AND provider_error_kind = 'cancelled'
            AND provider_outcome_certainty IN ('definitely_not_sent', 'outcome_unknown')
            AND billing_ambiguity = (provider_outcome_certainty = 'outcome_unknown')
        )
        OR (
            state = 'provider_outcome_unknown'
            AND provider_error_kind IN (
                'transport_after_possible_processing',
                'timeout_after_output',
                'cancelled',
                'provider_outcome_unknown'
            )
            AND provider_outcome_certainty = 'outcome_unknown'
            AND billing_ambiguity = 1
        )
    )
);
