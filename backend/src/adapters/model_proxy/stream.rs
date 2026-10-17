use super::{ModelComputeAuthority, unknown, wire};
use crate::distribution_contracts::wire::decode_message;
use crate::domain::*;
use crate::ports::{clock::Clock, model_provider::*};
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc, time::Duration};

pub(super) struct ProxyStream {
    response: reqwest::Response,
    request: Value,
    client: Option<Arc<tokio::sync::Mutex<crate::worker_auth::client::WorkerClient>>>,
    invocation: ModelProviderInvocation,
    clock: Arc<dyn Clock>,
    deadline: crate::ports::clock::MonotonicInstant,
    compute: Arc<dyn ModelComputeAuthority>,
    work: String,
    grant: String,
    pending: Vec<u8>,
    chunk: VecDeque<u8>,
    bytes: usize,
    sequence: u64,
    terminal: bool,
    done: bool,
    ready: VecDeque<ModelStreamEvent>,
    history: Vec<Value>,
    replay_at: usize,
    recovering: bool,
    recoveries: u8,
}
impl ProxyStream {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        response: reqwest::Response,
        request: Value,
        client: Arc<tokio::sync::Mutex<crate::worker_auth::client::WorkerClient>>,
        invocation: ModelProviderInvocation,
        clock: Arc<dyn Clock>,
        deadline: crate::ports::clock::MonotonicInstant,
        compute: Arc<dyn ModelComputeAuthority>,
        work: String,
        grant: String,
    ) -> Self {
        Self {
            response,
            request,
            client: Some(client),
            invocation,
            clock,
            deadline,
            compute,
            work,
            grant,
            pending: Vec::new(),
            chunk: VecDeque::new(),
            bytes: 0,
            sequence: 0,
            terminal: false,
            done: false,
            ready: VecDeque::new(),
            history: Vec::new(),
            replay_at: 0,
            recovering: false,
            recoveries: 0,
        }
    }
    fn ordinal(e: &Value) -> Result<u32, ProviderError> {
        wire::text(e, "item_ordinal")?
            .parse()
            .map_err(|_| unknown())
    }
    fn event(&mut self, v: Value) -> Result<(), ProviderError> {
        if self.terminal
            || v["type"] != "model_event"
            || v["request_id"] != self.request["request_id"]
            || v["authority"] != self.request["authority"]
            || v["environment"] != "development"
        {
            return Err(unknown());
        }
        let b = &v["body"];
        let inv = &self.request["body"]["invocation"];
        self.sequence += 1;
        if self.sequence > 4096
            || b["attempt_id"] != inv["attempt_id"]
            || b["logical_invocation_id"] != inv["logical_invocation_id"]
            || b["event_sequence"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                != Some(self.sequence)
        {
            return Err(unknown());
        }
        let e = &b["event"];
        let kind = e["kind"].as_str().ok_or_else(unknown)?;
        if self.sequence == 1 && kind != "response_started"
            || self.sequence > 1 && kind == "response_started"
        {
            return Err(unknown());
        }
        let target = self.invocation.request.target().identity();
        let event = match kind {
            "response_started" => {
                if e["model_config_id"] != inv["model_config_id"]
                    || e["model_config_digest"] != inv["model_config_digest"]
                {
                    return Err(unknown());
                }
                ModelStreamEvent::ResponseStarted {
                    target,
                    provider_request_id: None,
                    provider_response_id: None,
                }
            }
            "text_delta" | "reasoning_summary_delta" | "refusal_delta" => {
                let delta =
                    ModelTextPart::try_new(wire::text(e, "delta")?).map_err(|_| unknown())?;
                let item_ordinal = Self::ordinal(e)?;
                match kind {
                    "text_delta" => ModelStreamEvent::TextDelta {
                        item_ordinal,
                        delta,
                    },
                    "refusal_delta" => ModelStreamEvent::RefusalDelta {
                        item_ordinal,
                        delta,
                    },
                    _ => ModelStreamEvent::ReasoningSummaryDelta {
                        item_ordinal,
                        delta,
                    },
                }
            }
            "tool_call_started" => ModelStreamEvent::ToolCallStarted {
                item_ordinal: Self::ordinal(e)?,
                call_id: ModelToolCallId::try_new(wire::text(e, "call_id")?)
                    .map_err(|_| unknown())?,
                name: ToolName::try_new(wire::text(e, "name")?).map_err(|_| unknown())?,
            },
            "tool_argument_delta" => ModelStreamEvent::ToolArgumentDelta {
                item_ordinal: Self::ordinal(e)?,
                call_id: ModelToolCallId::try_new(wire::text(e, "call_id")?)
                    .map_err(|_| unknown())?,
                delta: wire::text(e, "delta")?,
            },
            "tool_call_completed" => ModelStreamEvent::ToolCallCompleted {
                item_ordinal: Self::ordinal(e)?,
                call: wire::call(e).map_err(|_| unknown())?,
            },
            "refusal_completed" => ModelStreamEvent::RefusalCompleted {
                item_ordinal: Self::ordinal(e)?,
            },
            "structured_data" => ModelStreamEvent::StructuredData {
                item_ordinal: Self::ordinal(e)?,
                data: wire::json_text(e, "data_json").map_err(|_| unknown())?,
            },
            "unknown_provider_event" => {
                ModelStreamEvent::UnknownProviderEvent(wire::opaque(e).map_err(|_| unknown())?)
            }
            "usage" => ModelStreamEvent::Usage(
                wire::usage(&e["outcome"])
                    .map_err(|_| unknown())?
                    .ok_or_else(unknown)?,
            ),
            "usage_unavailable" => ModelStreamEvent::UsageUnavailable,
            "completed" => {
                let output = e["output_items"]
                    .as_array()
                    .ok_or_else(unknown)?
                    .iter()
                    .map(wire::item)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| unknown())?;
                let usage = wire::usage(&e["usage"]).map_err(|_| unknown())?;
                self.ready.push_back(
                    usage.map_or(ModelStreamEvent::UsageUnavailable, ModelStreamEvent::Usage),
                );
                let stop_reason = match e["stop_reason"].as_str() {
                    Some("completed") => ModelStopReason::Completed,
                    Some("tool_continuation") => ModelStopReason::ToolContinuation,
                    Some("refusal") => ModelStopReason::Refusal,
                    Some("incomplete_provider_limit") => ModelStopReason::IncompleteProviderLimit,
                    _ => return Err(unknown()),
                };
                self.terminal = true;
                ModelStreamEvent::Completed(Box::new(
                    ModelResponse::try_new(ModelResponseInput {
                        selected_target: target,
                        output_items: output,
                        stop_reason,
                        usage,
                        provider_request_id: None,
                        provider_response_id: None,
                        provider_continuation: None,
                        provider_metadata: ProviderMetadata::default(),
                    })
                    .map_err(|_| unknown())?,
                ))
            }
            "provider_error" | "outcome_unknown" => return Err(unknown()),
            _ => return Err(unknown()),
        };
        self.ready.push_back(event);
        Ok(())
    }
    async fn read(&mut self) -> Result<bool, ProviderError> {
        loop {
            while let Some(b) = self.chunk.pop_front() {
                if self.terminal {
                    return Err(unknown());
                }
                if b == b'\n' {
                    if self.pending.is_empty() {
                        return Err(unknown());
                    }
                    let bytes = std::mem::take(&mut self.pending);
                    let v = decode_message(&bytes).map_err(|_| unknown())?;
                    let v = serde_json::to_value(v).map_err(|_| unknown())?;
                    if self.recovering && self.replay_at < self.history.len() {
                        if self.history[self.replay_at] != v {
                            return Err(unknown());
                        }
                        if v["body"]["event"]["kind"] == "completed" {
                            self.terminal = true;
                        }
                        self.replay_at += 1;
                        continue;
                    }
                    self.recovering = false;
                    self.event(v.clone())?;
                    self.history.push(v);
                    return Ok(true);
                }
                self.pending.push(b);
                if self.pending.len() > 65536 {
                    return Err(unknown());
                }
            }
            self.compute
                .check(&self.work, &self.grant)
                .map_err(|_| unknown())?;
            let remaining = self
                .invocation
                .control
                .absolute_deadline()
                .min(self.deadline)
                .checked_duration_since(self.clock.monotonic_now())
                .ok_or_else(unknown)?;
            let next = tokio::select! {biased;()=self.invocation.control.cancellation().cancelled()=>return Err(unknown()),v=tokio::time::timeout(remaining.min(Duration::from_secs(5)),self.response.chunk())=>v};
            let next = match next {
                Ok(Ok(chunk)) => chunk,
                _ => {
                    self.recover().await?;
                    continue;
                }
            };
            let Some(chunk) = next else {
                if !self.terminal {
                    self.recover().await?;
                    continue;
                }
                if !self.pending.is_empty() {
                    return Err(unknown());
                }
                self.done = true;
                return Ok(false);
            };
            self.bytes = self.bytes.checked_add(chunk.len()).ok_or_else(unknown)?;
            if self.bytes > 2097152 || chunk.len() > 131072 {
                return Err(unknown());
            }
            self.chunk.extend(chunk);
        }
    }
    async fn recover(&mut self) -> Result<(), ProviderError> {
        if self.recoveries >= 2 {
            return Err(unknown());
        }
        self.recoveries += 1;
        self.compute
            .check(&self.work, &self.grant)
            .map_err(|_| unknown())?;
        let remaining = self
            .deadline
            .checked_duration_since(self.clock.monotonic_now())
            .ok_or_else(unknown)?;
        let client = self.client.clone().ok_or_else(unknown)?;
        self.response = tokio::select! {biased;()=self.invocation.control.cancellation().cancelled()=>return Err(unknown()),r=tokio::time::timeout(remaining,super::status::recover(client,&self.request))=>r.map_err(|_|unknown())??};
        self.pending.clear();
        self.chunk.clear();
        self.bytes = 0;
        self.replay_at = 0;
        self.recovering = true;
        self.terminal = false;
        Ok(())
    }
}
impl ModelProviderStream for ProxyStream {
    fn next_event(&mut self) -> ModelProviderFuture<'_, Option<ModelStreamEvent>> {
        Box::pin(async move {
            self.compute
                .check(&self.work, &self.grant)
                .map_err(|_| unknown())?;
            if self.clock.monotonic_now() >= self.deadline
                || self.invocation.control.cancellation().is_cancelled()
            {
                return Err(unknown());
            }
            if self.done {
                return Ok(self.ready.pop_front());
            }
            if self.terminal {
                // Completion is released only after EOF proves no contradictory trailing frame.
                if self.read().await? {
                    return Err(unknown());
                }
                return Ok(self.ready.pop_front());
            }
            if let Some(e) = self.ready.pop_front() {
                return Ok(Some(e));
            }
            if let Err(error) = self.read().await {
                if self.sequence <= 2 {
                    return Err(if let Some(c) = &self.client {
                        super::status::failure(c.clone(), &self.request).await
                    } else {
                        unknown()
                    });
                }
                return Err(error);
            }
            if self.terminal && self.read().await? {
                return Err(unknown());
            }
            Ok(self.ready.pop_front())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    struct Allow;
    impl ModelComputeAuthority for Allow {
        fn check(&self, _: &str, _: &str) -> Result<(), ProviderError> {
            Ok(())
        }
    }
    fn q() -> Value {
        let u4 = "11111111-1111-4111-8111-111111111111";
        let u7 = "019e0000-0000-7000-8000-000000000001";
        json!({"request_id":u4,"authority":{"customer_id":u4,"logical_agent_id":u4,"installation_id":u4,"generation":"1","session_id":u4},"body":{"invocation":{"attempt_id":u7,"logical_invocation_id":u7,"model_config_id":u4,"model_config_digest":"a".repeat(64)}}})
    }
    fn frames() -> Vec<Value> {
        let q = q();
        let inv = &q["body"]["invocation"];
        [json!({"kind":"response_started","model_config_id":inv["model_config_id"],"model_config_digest":inv["model_config_digest"],"reservation_id":q["request_id"]}),
         json!({"kind":"text_delta","item_ordinal":"0","delta":"😀 exact"}),
         json!({"kind":"completed","output_items":[{"kind":"text","content_parts":["😀 exact"]}],"stop_reason":"completed","usage":{"usage_status":"unknown","reservation_id":q["request_id"],"liability":"retained"}})]
         .into_iter().enumerate().map(|(i,event)|json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"model_event","request_id":q["request_id"],"environment":"development","authority":q["authority"],"body":{"attempt_id":inv["attempt_id"],"logical_invocation_id":inv["logical_invocation_id"],"event_sequence":(i+1).to_string(),"event":event}})).collect()
    }
    fn encode(frames: &[Value]) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|v| {
                let mut b = serde_json::to_vec(v).unwrap();
                b.push(b'\n');
                b
            })
            .collect()
    }
    async fn consume_prefix(
        bytes: Vec<u8>,
        cancelled: bool,
        expired: bool,
        prefix: Vec<Value>,
    ) -> (bool, bool, usize) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let size = bytes.len();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request);
            if write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
            .is_ok()
            {
                for chunk in bytes.chunks(17) {
                    if socket.write_all(chunk).is_err() {
                        break;
                    }
                }
            }
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap();
        let clock = Arc::new(crate::adapters::system_clock::SystemClock::new());
        let token = ProviderCancellationToken::new();
        if cancelled {
            token.cancel();
        }
        let invocation = ModelProviderInvocation {
            request: super::super::tests::request(),
            attempt: ProviderAttempt::try_new(1).unwrap(),
            control: ModelInvocationControl::try_new(
                token,
                clock
                    .monotonic_now()
                    .checked_add(Duration::from_secs(2))
                    .unwrap(),
                Duration::from_secs(1),
            )
            .unwrap(),
            fixture_key: None,
        };
        let mut stream = ProxyStream {
            response,
            request: q(),
            client: None,
            invocation,
            deadline: if expired {
                crate::ports::clock::MonotonicInstant::from_elapsed(Duration::ZERO)
            } else {
                clock
                    .monotonic_now()
                    .checked_add(Duration::from_secs(30))
                    .unwrap()
            },
            clock,
            compute: Arc::new(Allow),
            work: "work".into(),
            grant: "grant".into(),
            pending: Vec::new(),
            chunk: VecDeque::new(),
            bytes: 0,
            sequence: 0,
            terminal: false,
            done: false,
            ready: VecDeque::new(),
            history: Vec::new(),
            replay_at: 0,
            recovering: false,
            recoveries: 0,
        };
        let mut completed = false;
        let mut failed = false;
        let mut text_events = 0;
        for event in &prefix {
            stream.event(event.clone()).unwrap();
        }
        stream.history = prefix;
        stream.recovering = !stream.history.is_empty();
        if stream.recovering {
            stream.terminal = false;
        }
        loop {
            match stream.next_event().await {
                Ok(Some(ModelStreamEvent::Completed(_))) => completed = true,
                Ok(Some(ModelStreamEvent::TextDelta { .. })) => text_events += 1,
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        drop(stream);
        thread.join().unwrap();
        (completed, failed, text_events)
    }
    async fn consume_with_deadline(bytes: Vec<u8>, cancelled: bool, expired: bool) -> (bool, bool) {
        let (completed, failed, _) = consume_prefix(bytes, cancelled, expired, Vec::new()).await;
        (completed, failed)
    }
    async fn consume(bytes: Vec<u8>, cancelled: bool) -> (bool, bool) {
        consume_with_deadline(bytes, cancelled, false).await
    }
    #[tokio::test]
    async fn original_stream_deadline_applies_even_to_immediately_buffered_frames() {
        assert_eq!(
            consume_with_deadline(encode(&frames()), false, true).await,
            (false, true)
        );
    }
    #[tokio::test]
    async fn fragmented_terminal_is_released_only_after_clean_eof() {
        assert_eq!(consume(encode(&frames()), false).await, (true, false));
    }
    #[tokio::test]
    async fn committed_replay_checks_consumed_prefix_without_duplicate_semantic_events() {
        let base = frames();
        assert_eq!(
            consume_prefix(encode(&base), false, false, base[..2].to_vec()).await,
            (true, false, 1)
        );
        assert_eq!(
            consume_prefix(encode(&base), false, false, base.clone()).await,
            (true, false, 1)
        );
        let mut changed = base.clone();
        changed[1]["body"]["event"]["delta"] = json!("conflicting immutable output");
        assert_eq!(
            consume_prefix(encode(&changed), false, false, base[..2].to_vec()).await,
            (false, true, 1)
        );
    }
    #[tokio::test]
    async fn malformed_incomplete_or_contradictory_stream_never_completes() {
        let base = frames();
        let mut cases = vec![
            encode(&base[..2]),
            b"{bad}\n".to_vec(),
            vec![0xff, b'\n'],
            vec![b' '; 65537],
        ];
        let mut duplicate = base.clone();
        duplicate.push(base[2].clone());
        cases.push(encode(&duplicate));
        let mut no_lf = encode(&base);
        no_lf.pop();
        cases.push(no_lf);
        for (path, value) in [
            ("attempt_id", json!("019e0000-0000-7000-8000-000000000002")),
            ("event_sequence", json!("9")),
        ] {
            let mut f = base.clone();
            f[1]["body"][path] = value;
            cases.push(encode(&f));
        }
        let mut f = base.clone();
        f[0]["body"]["event"]["model_config_id"] = json!("22222222-2222-4222-8222-222222222222");
        cases.push(encode(&f));
        let mut f = base.clone();
        f[1]["body"]["event"]["kind"] = json!("unsupported");
        cases.push(encode(&f));
        for bytes in cases {
            assert_eq!(consume(bytes, false).await, (false, true));
        }
        assert_eq!(consume(encode(&base), true).await, (false, true));
    }
}
