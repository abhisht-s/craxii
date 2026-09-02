#![cfg(unix)]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::time::Duration;

use craxii_server::adapters::scripted_provider::ScriptGate;
use craxii_server::domain::{ClientCommandId, ClientMessageId, WorkId};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use sqlx::Connection as _;
use stage18_harness::{
    EstimatorMode, ProgramPlan, Stage18Harness, Stage18Root, ToolPlan,
    gated_after_delta_answer_program, gated_after_delta_refusal_program,
    gated_mixed_text_tool_programs, programs,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn client_message_id() -> ClientMessageId {
    format!("{:08x}-0000-7000-8000-000000000020", std::process::id())
        .parse()
        .unwrap_or_else(|_| {
            ClientMessageId::parse_canonical("01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c20").unwrap()
        })
}

fn client_command_id() -> ClientCommandId {
    "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c21".parse().unwrap()
}

async fn connect(harness: &Stage18Harness, after: u64) -> Socket {
    let url = format!("ws://{}/v1/events?after={after}", harness.authority);
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", harness.bearer).parse().unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn next_json(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("live frame timeout")
            .expect("live socket closed")
            .expect("live frame error");
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                return serde_json::from_str(&text).unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Pong(bytes))
                    .await
                    .unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Pong(_) => {}
            other => panic!("unexpected live frame: {other:?}"),
        }
    }
}

async fn through_sync(socket: &mut Socket) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_json(socket).await;
        let complete = frame["event_type"] == "sync.complete";
        frames.push(frame);
        if complete {
            return frames;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_gateway_draft_reconciles_to_durable_commit_and_reconnect_replays_no_draft() {
    let gate = ScriptGate::new();
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-text"),
        gated_after_delta_answer_program("authoritative final answer", gate.clone()),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let mut socket = connect(&harness, 0).await;
    let initial = through_sync(&mut socket).await;
    assert!(initial.iter().all(|frame| {
        !frame["event_type"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assistant.draft_")
    }));

    let accepted = harness
        .submit_message("stream a safe answer", client_message_id())
        .await;
    assert_eq!(accepted.status, 202);
    let work_id: WorkId = accepted.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let draft_id = loop {
        let frame = next_json(&mut socket).await;
        if frame["event_type"] == "assistant.draft_delta" {
            assert_eq!(frame["delivery_kind"], "ephemeral");
            assert!(frame["cursor"].is_null());
            assert_eq!(
                frame["conversation_id"],
                harness.identity.conversation_id.to_string()
            );
            assert_eq!(frame["work_id"], work_id.to_string());
            assert_eq!(frame["delta_sequence"], 1);
            assert_eq!(frame["payload"]["kind"], "text");
            assert_eq!(frame["payload"]["text"], "authoritative final answer");
            break frame["draft_id"].as_str().map(str::to_owned);
        }
    };
    assert!(draft_id.is_some());
    gate.release();

    let final_cursor = loop {
        let frame = next_json(&mut socket).await;
        if frame["event_type"] == "assistant.message_committed" {
            assert_eq!(frame["delivery_kind"], "durable");
            assert_eq!(frame["work_id"], work_id.to_string());
            assert_eq!(
                frame["payload"]["content"][0]["text"],
                "authoritative final answer"
            );
            break frame["cursor"].as_u64().unwrap();
        }
    };
    assert_eq!(harness.wait_terminal(work_id).await, "completed");
    socket.close(None).await.unwrap();

    let mut reconnect = connect(&harness, 0).await;
    let replay = through_sync(&mut reconnect).await;
    assert!(replay.iter().any(|frame| {
        frame["event_type"] == "assistant.message_committed"
            && frame["cursor"]
                .as_u64()
                .is_some_and(|cursor| cursor <= final_cursor)
    }));
    assert!(replay.iter().all(|frame| {
        !frame["event_type"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assistant.draft_")
    }));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), reconnect.next())
            .await
            .is_err()
    );
    reconnect.close(None).await.unwrap();

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(harness.root.database())
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let exposed: i64 =
        sqlx::query_scalar("SELECT draft_exposed FROM model_invocations WHERE work_id = ? LIMIT 1")
            .bind(work_id.to_string())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(exposed, 1);
    let persisted_draft_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name LIKE '%draft%'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(persisted_draft_tables, 0);
    connection.close().await.unwrap();
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusal_delta_is_public_but_refused_completion_remains_durable_authority() {
    let gate = ScriptGate::new();
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-refusal"),
        gated_after_delta_refusal_program("safe refusal", gate.clone()),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let mut socket = connect(&harness, 0).await;
    through_sync(&mut socket).await;
    let accepted = harness
        .submit_message("request a refusal", client_message_id())
        .await;
    let work_id: WorkId = accepted.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    loop {
        let frame = next_json(&mut socket).await;
        if frame["event_type"] == "assistant.draft_delta" {
            assert_eq!(frame["payload"]["kind"], "refusal");
            assert_eq!(frame["payload"]["text"], "safe refusal");
            break;
        }
    }
    gate.release();
    loop {
        let frame = next_json(&mut socket).await;
        if frame["event_type"] == "assistant.message_committed" {
            assert_eq!(frame["payload"]["content"][0]["text"], "safe refusal");
            break;
        }
    }
    assert_eq!(harness.wait_terminal(work_id).await, "completed");
    socket.close(None).await.unwrap();
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_drops_draft_while_work_completes_and_reconnect_gets_only_durable_final() {
    let gate = ScriptGate::new();
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-disconnect"),
        gated_after_delta_answer_program("final after disconnect", gate.clone()),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let mut socket = connect(&harness, 0).await;
    through_sync(&mut socket).await;
    let accepted = harness
        .submit_message("disconnect during draft", client_message_id())
        .await;
    let work_id: WorkId = accepted.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    loop {
        if next_json(&mut socket).await["event_type"] == "assistant.draft_delta" {
            break;
        }
    }
    socket.close(None).await.unwrap();
    gate.release();
    assert_eq!(harness.wait_terminal(work_id).await, "completed");

    let mut reconnect = connect(&harness, 0).await;
    let replay = through_sync(&mut reconnect).await;
    assert!(replay.iter().all(|frame| {
        !frame["event_type"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assistant.draft_")
    }));
    assert!(replay.iter().any(|frame| {
        frame["event_type"] == "assistant.message_committed"
            && frame["payload"]["content"][0]["text"] == "final after disconnect"
    }));
    reconnect.close(None).await.unwrap();
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_text_tool_is_abandoned_before_safe_tool_progress_and_new_invocation_draft() {
    let first_gate = ScriptGate::new();
    let final_gate = ScriptGate::new();
    let programs = gated_mixed_text_tool_programs(
        "preliminary text that must not commit",
        ToolPlan::new(
            "stage20-tool",
            "run_shell",
            serde_json::json!({"command": "printf private-tool-result"}),
        ),
        first_gate.clone(),
        "final answer after tool",
        final_gate.clone(),
    );
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-tool"),
        programs,
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let mut socket = connect(&harness, 0).await;
    through_sync(&mut socket).await;
    let accepted = harness
        .submit_message("use one tool safely", client_message_id())
        .await;
    let work_id: WorkId = accepted.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let first_draft = loop {
        let frame = next_json(&mut socket).await;
        if frame["event_type"] == "assistant.draft_delta" {
            assert_eq!(
                frame["payload"]["text"],
                "preliminary text that must not commit"
            );
            break frame["draft_id"].as_str().map(str::to_owned);
        }
    };
    first_gate.release();

    let mut abandoned = false;
    let mut tool_started = false;
    let mut tool_finished = false;
    let second_draft = loop {
        let frame = next_json(&mut socket).await;
        let encoded = serde_json::to_string(&frame).unwrap();
        assert!(!encoded.contains("private-tool-result"));
        match frame["event_type"].as_str() {
            Some("assistant.draft_abandoned") => {
                assert_eq!(frame["payload"]["reason"], "tool_continuation");
                abandoned = true;
            }
            Some("tool.execution_started") => tool_started = true,
            Some("tool.execution_finished") => tool_finished = true,
            Some("assistant.draft_delta")
                if frame["payload"]["text"] == "final answer after tool" =>
            {
                break frame["draft_id"].as_str().unwrap().to_owned();
            }
            _ => {}
        }
    };
    assert!(abandoned);
    assert_ne!(first_draft.unwrap(), second_draft);
    final_gate.release();

    loop {
        let frame = next_json(&mut socket).await;
        let encoded = serde_json::to_string(&frame).unwrap();
        assert!(!encoded.contains("private-tool-result"));
        match frame["event_type"].as_str() {
            Some("tool.execution_started") => tool_started = true,
            Some("tool.execution_finished") => tool_finished = true,
            Some("assistant.message_committed") => {
                assert_eq!(
                    frame["payload"]["content"][0]["text"],
                    "final answer after tool"
                );
                assert!(!encoded.contains("preliminary text that must not commit"));
                break;
            }
            _ => {}
        }
    }
    assert!(tool_started && tool_finished);
    assert_eq!(harness.wait_terminal(work_id).await, "completed");
    socket.close(None).await.unwrap();
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_visible_delta_abandons_without_transport_affecting_work() {
    let gate = ScriptGate::new();
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-cancel"),
        gated_after_delta_answer_program("cancel this draft", gate),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let mut socket = connect(&harness, 0).await;
    through_sync(&mut socket).await;
    let accepted = harness
        .submit_message("cancel while streaming", client_message_id())
        .await;
    let work_id: WorkId = accepted.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    loop {
        if next_json(&mut socket).await["event_type"] == "assistant.draft_delta" {
            break;
        }
    }
    let cancelled = harness.cancel_work(work_id, client_command_id()).await;
    assert!(matches!(cancelled.status, 200 | 202));
    let mut saw_abandon = false;
    for _ in 0..32 {
        let frame = next_json(&mut socket).await;
        if frame["event_type"] == "assistant.draft_abandoned" {
            assert_eq!(frame["payload"]["reason"], "cancelled");
            saw_abandon = true;
        }
        if matches!(
            frame["event_type"].as_str(),
            Some("work.cancelled" | "work.interrupted" | "work.failed")
        ) {
            break;
        }
    }
    assert!(saw_abandon);
    assert!(matches!(
        harness.wait_terminal(work_id).await.as_str(),
        "cancelled" | "interrupted"
    ));
    socket.close(None).await.unwrap();
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_route_still_requires_stage11_bearer_authentication() {
    let gate = ScriptGate::new();
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-auth"),
        gated_after_delta_answer_program("unused", gate),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let url = format!("ws://{}/v1/events?after=0", harness.authority);
    let error = tokio_tungstenite::connect_async(url).await.unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("missing bearer must fail as HTTP auth rejection");
    };
    assert_eq!(response.status(), 401);
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_live_client_preserves_canonical_execution_and_bootstrap_result() {
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-no-client"),
        programs(&[ProgramPlan::Answer {
            text: "canonical without live client".to_owned(),
            require_tool_result: None,
        }]),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let accepted = harness
        .submit_message("complete without WebSocket", client_message_id())
        .await;
    let work_id: WorkId = accepted.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(harness.wait_terminal(work_id).await, "completed");
    let bootstrap = harness.bootstrap().await.json();
    assert!(
        bootstrap["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["work_id"] == work_id.to_string()
                    && message["content"][0]["text"] == "canonical without live client"
            })
    );
    assert_eq!(harness.live_events.metrics().active_connections, 0);
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_shutdown_closes_synced_live_connection_1001_and_joins_transport() {
    let gate = ScriptGate::new();
    let harness = Stage18Harness::start(
        Stage18Root::new("stage20-shutdown"),
        gated_after_delta_answer_program("unused", gate),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let mut socket = connect(&harness, 0).await;
    through_sync(&mut socket).await;
    let shutdown = tokio::spawn(harness.shutdown());
    let close = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("shutdown close timeout")
        .expect("socket must produce a close result")
        .expect("shutdown close frame");
    let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = close else {
        panic!("expected owned shutdown close frame, got {close:?}");
    };
    assert_eq!(u16::from(frame.code), 1001);
    shutdown.await.unwrap();
}
