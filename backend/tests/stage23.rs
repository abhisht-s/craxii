#![cfg(all(feature = "test-failpoints", unix))]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use craxii_server::adapters::telemetry;
use craxii_server::bootstrap::config;
use craxii_server::domain::{ClientCommandId, ClientMessageId, WorkId};
use serde_json::Value;
use stage18_harness::{
    EstimatorMode, ProgramPlan, Stage18Harness, Stage18Root, programs, retry_programs,
};

const USER_CONTENT_SENTINEL: &str = "SENTINEL_USER_MESSAGE_23_REAL_TRACE";
const MODEL_OUTPUT_SENTINEL: &str = "SENTINEL_MODEL_OUTPUT_23_REAL_TRACE";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_json_reconstructs_request_command_work_and_model_attempt() {
    let input = include_str!("fixtures/config/valid/local.toml")
        .replace("format = \"pretty\"", "format = \"json\"");
    let configuration = config::parse(&input).expect("valid production-style tracing config");
    let (dispatch, capture) = telemetry::production_test_dispatch(configuration.tracing());
    tracing::dispatcher::set_global_default(dispatch)
        .expect("Stage 23 integration test owns its process-global subscriber");

    let root = Stage18Root::new("stage23-production-trace");
    let harness = Stage18Harness::start(
        root,
        programs(&[ProgramPlan::Answer {
            text: MODEL_OUTPUT_SENTINEL.to_owned(),
            require_tool_result: None,
        }]),
        EstimatorMode::Normal,
    )
    .await
    .expect("start real HTTP/runtime/model harness");
    let bearer = harness.bearer.clone();
    let conversation_id = harness.identity.conversation_id.to_string();
    let client_message_id =
        ClientMessageId::parse_canonical("01890f6c-7b3a-7cc0-98f1-02e6f7a8b923").unwrap();

    let accepted = harness
        .submit_message(USER_CONTENT_SENTINEL, client_message_id)
        .await;
    assert_eq!(accepted.status, 202);
    let accepted_body = accepted.json();
    let message_id = accepted_body["message_id"].as_str().unwrap().to_owned();
    let work_id: WorkId = accepted_body["work_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(harness.wait_terminal(work_id).await, "completed");

    let retransmission = harness
        .submit_message(USER_CONTENT_SENTINEL, client_message_id)
        .await;
    assert_eq!(retransmission.status, 202);
    assert_eq!(retransmission.json()["duplicate"], true);

    let cancellation_command_id =
        ClientCommandId::parse_canonical("01890f6c-7b3a-7cc0-98f1-02e6f7a8b924").unwrap();
    let cancellation = harness.cancel_work(work_id, cancellation_command_id).await;
    assert!(matches!(cancellation.status, 200 | 202));
    assert_eq!(cancellation.json()["work_id"], work_id.to_string());
    let cancellation_replay = harness.cancel_work(work_id, cancellation_command_id).await;
    assert!(matches!(cancellation_replay.status, 200 | 202));
    assert_eq!(cancellation_replay.json()["duplicate"], true);

    let root = harness.shutdown().await;

    let retry_harness = Stage18Harness::start(
        Stage18Root::new("stage23-production-retry-trace"),
        retry_programs(MODEL_OUTPUT_SENTINEL, 2),
        EstimatorMode::Normal,
    )
    .await
    .expect("start real retry telemetry harness");
    let retry_client_message_id =
        ClientMessageId::parse_canonical("01890f6c-7b3a-7cc0-98f1-02e6f7a8b925").unwrap();
    let retry_response = retry_harness
        .submit_message(USER_CONTENT_SENTINEL, retry_client_message_id)
        .await;
    assert_eq!(retry_response.status, 202);
    let retry_work_id: WorkId = retry_response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        retry_harness.wait_terminal(retry_work_id).await,
        "completed"
    );
    let retry_root = retry_harness.shutdown().await;

    let output = capture.output();
    let records: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).expect("production JSON telemetry record"))
        .collect();

    let accepted_command = find_event(&records, "client_command_terminal", |record| {
        record["command_kind"] == "message"
            && record["client_message_id"] == client_message_id.to_string()
            && record["result_class"] == "accepted"
    });
    assert_eq!(accepted_command["message_id"], message_id);
    assert_eq!(accepted_command["work_id"], work_id.to_string());
    assert_eq!(accepted_command["conversation_id"], conversation_id);
    assert!(accepted_command["duration_micros"].is_number());
    assert!(accepted_command["commit_to_response_micros"].is_number());
    assert_span_path(accepted_command, &["http_request", "client_command"]);

    let request_id = accepted_command["request_id"]
        .as_str()
        .expect("explicit command request ID");
    let request_terminal = find_event(&records, "http_request_terminal", |record| {
        record["request_id"] == request_id && record["status"] == 202
    });
    assert_eq!(request_terminal["result_class"], "success");

    let work_terminal = find_event(&records, "work_terminal", |record| {
        record["work_id"] == work_id.to_string()
    });
    assert_span_path(work_terminal, &["work_execution"]);

    let model_terminal = find_event(&records, "model_attempt_terminal", |record| {
        record["work_id"] == work_id.to_string() && record["result_class"] == "completed"
    });
    for field in [
        "logical_invocation_id",
        "model_invocation_id",
        "attempt_ordinal",
        "provider",
        "model",
        "target",
        "request_sha256",
        "request_bytes",
        "total_latency_ms",
        "certainty",
        "stop_reason",
        "output_item_count",
        "tool_call_count",
        "usage_status",
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "total_tokens",
        "provider_request_digest",
        "provider_response_digest",
    ] {
        assert!(
            model_terminal.get(field).is_some(),
            "model terminal event omitted explicit {field}: {model_terminal}"
        );
    }
    assert_eq!(model_terminal["attempt_ordinal"], 1);
    assert_eq!(model_terminal["certainty"], "definitely_completed");
    assert_eq!(model_terminal["usage_status"], "reported");
    assert!(
        model_terminal["provider_request_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_span_path(
        model_terminal,
        &["work_execution", "model_invocation_attempt"],
    );

    let failed_retry_attempt = find_event(&records, "model_attempt_terminal", |record| {
        record["work_id"] == retry_work_id.to_string()
            && record["attempt_ordinal"] == 1
            && record["result_class"] == "failed"
    });
    let retry_scheduled = find_event(&records, "model_attempt_retry_scheduled", |record| {
        record["work_id"] == retry_work_id.to_string()
    });
    assert_eq!(
        retry_scheduled["model_invocation_id"],
        failed_retry_attempt["model_invocation_id"]
    );
    assert_eq!(
        retry_scheduled["logical_invocation_id"],
        failed_retry_attempt["logical_invocation_id"]
    );
    assert_eq!(
        retry_scheduled["retry_reason"],
        "classified_transient_before_output"
    );
    assert!(retry_scheduled["retry_delay_ms"].is_number());
    assert_span_path(
        retry_scheduled,
        &["work_execution", "model_invocation_attempt"],
    );
    let completed_retry_attempt = find_event(&records, "model_attempt_terminal", |record| {
        record["work_id"] == retry_work_id.to_string()
            && record["attempt_ordinal"] == 2
            && record["result_class"] == "completed"
    });
    assert_eq!(
        completed_retry_attempt["retry_of_invocation_id"],
        failed_retry_attempt["model_invocation_id"]
    );
    assert_eq!(
        completed_retry_attempt["retry_reason"],
        "classified_transient_before_output"
    );

    let retransmitted_command = find_event(&records, "client_command_terminal", |record| {
        record["command_kind"] == "message"
            && record["client_message_id"] == client_message_id.to_string()
            && record["result_class"] == "retransmission"
    });
    assert_eq!(retransmitted_command["work_id"], work_id.to_string());

    for result_class in ["accepted", "retransmission"] {
        let cancellation_event = find_event(&records, "client_command_terminal", |record| {
            record["command_kind"] == "cancellation"
                && record["cancellation_command_id"] == cancellation_command_id.to_string()
                && record["result_class"] == result_class
        });
        assert_eq!(cancellation_event["work_id"], work_id.to_string());
        assert!(cancellation_event["resulting_work_state"].is_string());
        assert!(cancellation_event["journal_cursor"].is_number());
        assert!(cancellation_event["cleanup_pending"].is_boolean());
        assert_span_path(cancellation_event, &["http_request", "client_command"]);
    }

    for forbidden in [
        USER_CONTENT_SENTINEL,
        MODEL_OUTPUT_SENTINEL,
        bearer.as_str(),
        "stage18-req-1",
        "stage18-resp-1",
        "stage18-req-2",
        "stage18-resp-2",
        "Authorization",
    ] {
        assert!(
            !output.contains(forbidden),
            "production trace leaked {forbidden}"
        );
    }

    root.remove();
    retry_root.remove();
}

fn find_event<'a>(
    records: &'a [Value],
    event_name: &str,
    predicate: impl Fn(&Value) -> bool,
) -> &'a Value {
    records
        .iter()
        .find(|record| record["event_name"] == event_name && predicate(record))
        .unwrap_or_else(|| panic!("missing {event_name} in production JSON telemetry"))
}

fn assert_span_path(record: &Value, required: &[&str]) {
    let mut names: Vec<&str> = record["spans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|span| span["name"].as_str())
        .collect();
    if let Some(current) = record["span"]["name"].as_str() {
        names.push(current);
    }
    for name in required {
        assert!(names.contains(name), "missing span {name} in {record}");
    }
}
