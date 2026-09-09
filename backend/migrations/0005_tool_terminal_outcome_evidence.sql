-- V4 allowed definite post-dispatch terminal rows to retain NULL interruption flags. Repair only
-- legacy rows whose non-NULL evidence is already compatible with the canonical result class; an
-- actually contradictory row must make the migration fail closed.
UPDATE tool_executions
SET timed_out = CASE json_extract(result_json, '$.result_kind')
        WHEN 'timeout' THEN 1
        ELSE 0
    END,
    cancelled = CASE json_extract(result_json, '$.result_kind')
        WHEN 'cancellation' THEN 1
        ELSE 0
    END
WHERE state = 'completed'
  AND dispatch_intent_at IS NOT NULL
  AND (timed_out IS NULL OR cancelled IS NULL)
  AND coalesce(
        timed_out,
        CASE json_extract(result_json, '$.result_kind') WHEN 'timeout' THEN 1 ELSE 0 END
      ) = CASE json_extract(result_json, '$.result_kind') WHEN 'timeout' THEN 1 ELSE 0 END
  AND coalesce(
        cancelled,
        CASE json_extract(result_json, '$.result_kind') WHEN 'cancellation' THEN 1 ELSE 0 END
      ) = CASE json_extract(result_json, '$.result_kind') WHEN 'cancellation' THEN 1 ELSE 0 END;

-- As in V4's model-attempt evidence contract, an additive column carries the cross-column
-- invariant for every future insert and update without rebuilding the foreign-key graph.
ALTER TABLE tool_executions ADD COLUMN terminal_evidence_version INTEGER NOT NULL DEFAULT 1
CHECK (
    terminal_evidence_version = 1
    AND (
        (
            state IN ('requested', 'dispatching', 'interrupted_before_dispatch', 'outcome_unknown')
            AND timed_out IS NULL
            AND cancelled IS NULL
        )
        OR (
            state = 'completed'
            AND dispatch_intent_at IS NULL
            AND timed_out IS NULL
            AND cancelled IS NULL
        )
        OR (
            state = 'completed'
            AND dispatch_intent_at IS NOT NULL
            AND timed_out IS NOT NULL
            AND cancelled IS NOT NULL
            AND coalesce((
                (
                    json_extract(result_json, '$.result_kind') = 'timeout'
                    AND timed_out = 1
                    AND cancelled = 0
                )
                OR (
                    json_extract(result_json, '$.result_kind') = 'cancellation'
                    AND timed_out = 0
                    AND cancelled = 1
                )
                OR (
                    json_extract(result_json, '$.result_kind') IN (
                        'success',
                        'validation_rejection',
                        'unknown_tool',
                        'authority_denial',
                        'file_error',
                        'process_exit',
                        'signal_termination',
                        'spawn_failure',
                        'cleanup_failure'
                    )
                    AND timed_out = 0
                    AND cancelled = 0
                )
            ), 0)
        )
    )
);
