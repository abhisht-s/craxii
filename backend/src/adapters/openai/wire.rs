use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(super) struct ResponsesRequest<'a> {
    pub(super) model: &'a str,
    pub(super) instructions: String,
    pub(super) input: Vec<Value>,
    pub(super) tools: Vec<FunctionTool<'a>>,
    pub(super) tool_choice: &'static str,
    pub(super) parallel_tool_calls: bool,
    pub(super) store: bool,
    pub(super) stream: bool,
    pub(super) truncation: &'static str,
    pub(super) max_output_tokens: u64,
}

#[derive(Serialize)]
pub(super) struct FunctionTool<'a> {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) name: &'a str,
    pub(super) description: &'a str,
    pub(super) parameters: &'a Value,
    pub(super) strict: bool,
}

#[derive(Deserialize)]
pub(super) struct EventEnvelope {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) sequence_number: u64,
}

#[derive(Deserialize)]
pub(super) struct Usage {
    pub(super) input_tokens: u64,
    #[serde(default)]
    pub(super) input_tokens_details: InputTokenDetails,
    pub(super) output_tokens: u64,
    #[serde(default)]
    pub(super) output_tokens_details: OutputTokenDetails,
    pub(super) total_tokens: u64,
}

#[derive(Default, Deserialize)]
pub(super) struct InputTokenDetails {
    #[serde(default)]
    pub(super) cached_tokens: u64,
}

#[derive(Default, Deserialize)]
pub(super) struct OutputTokenDetails {
    #[serde(default)]
    pub(super) reasoning_tokens: u64,
}
