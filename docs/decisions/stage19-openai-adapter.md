# Stage 19 OpenAI adapter decision

Date: 2026-09-02

- Craxii calls `POST /v1/responses` directly through Reqwest rather than adopting a provider SDK. OpenAI wire types remain under `backend/src/adapters/openai`.
- Reqwest is pinned by the lockfile at 0.13.4 with `default-features = false` and only `rustls`; the client disables redirects, proxy discovery, transport retries, and verbose connection logging. The locked Rustls graph adds reviewed permissive ISC, MIT-0, and CDLA-Permissive-2.0 license expressions to the repository allowlist rather than introducing a native OpenSSL dependency.
- Every invocation sends Craxii's complete canonical context with the exact configured model and output limit, `store=false`, `stream=true`, `truncation="disabled"`, and `parallel_tool_calls=false`. No conversation, previous-response, built-in tool, or fallback-model state is used.
- Stage 14 custom tool schemas remain authoritative. They are sent with `strict=false`; completed calls retain OpenAI `call_id`, require complete bounded JSON arguments, and are still validated by ToolExecutionService before execution.
- The canonical request has no response-schema field, so the initial OpenAI target must not advertise structured output and text is never reclassified merely because it parses as JSON.
- Current OpenAI documentation says encrypted reasoning content is returned by default for stateless `store=false` Responses requests. Explicit summaries alone become `ReasoningSummary`; encrypted items remain bounded opaque evidence and are never logged or interpreted. The standard production fixtures keep native continuation disabled, though the provider-guarded replay contract remains tested.
- `X-Client-Request-Id` is diagnostic correlation only. OpenAI documents no Responses idempotency guarantee, so Craxii assumes none; ModelGateway remains the only retry authority.
- Local validation before invocation is definitely unsent. Cancellation, timeout, malformed/truncated streaming, or transport loss after invocation begins is outcome-unknown unless an explicit provider terminal response proves otherwise, preventing duplicate automatic calls.
- Credentials are loaded once from the configured bounded regular-file/systemd credential source into `SecretString`. Missing credentials leave production `live_unready`; unsafe or malformed credentials fail with a redacted startup code.
- Readiness requires successful recovery, adapter/gateway/AgentLoop construction, runner installation, scheduler start, and initial scan. Startup never spends an OpenAI token to probe readiness.
- The ignored live smoke is explicit and excluded from `scripts/verify`. Missing credentials/model access produce `STAGE_19_LIVE_OPENAI_SMOKE: NOT_CONFIGURED`; implementation completion may record `DEFERRED_REQUIRES_OPENAI_API_KEY`.
