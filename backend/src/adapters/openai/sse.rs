use std::collections::VecDeque;

use crate::ports::model_provider::{ProviderError, ProviderErrorKind, ProviderOutcomeCertainty};

pub(super) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_SSE_EVENT_BYTES: usize = 512 * 1024;
pub(super) const MAX_SSE_EVENTS: usize = 4_096;

pub(super) struct SseEvent {
    pub(super) event: Option<String>,
    pub(super) data: String,
}

#[derive(Default)]
pub(super) struct SseDecoder {
    buffer: Vec<u8>,
    pub(super) total_bytes: usize,
    event_name: Option<String>,
    data_lines: Vec<String>,
    event_bytes: usize,
    event_count: usize,
    ready: VecDeque<SseEvent>,
}

impl SseDecoder {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(too_large)?;
        if self.total_bytes > MAX_STREAM_BYTES {
            return Err(too_large());
        }
        self.buffer.extend_from_slice(bytes);
        self.consume_lines(false)?;
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err(too_large());
        }
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<(), ProviderError> {
        self.consume_lines(true)?;
        if self.event_name.is_some() || !self.data_lines.is_empty() {
            self.dispatch()?;
        }
        Ok(())
    }

    fn consume_lines(&mut self, eof: bool) -> Result<(), ProviderError> {
        loop {
            let newline = self.buffer.iter().position(|byte| *byte == b'\n');
            let take = match (newline, eof && !self.buffer.is_empty()) {
                (Some(index), _) => index + 1,
                (None, true) => self.buffer.len(),
                (None, false) => break,
            };
            let mut line = self.buffer.drain(..take).collect::<Vec<_>>();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line).map_err(|_| malformed())?;
            if line.is_empty() {
                if self.event_name.is_some() || !self.data_lines.is_empty() {
                    self.dispatch()?;
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            self.event_bytes = self
                .event_bytes
                .checked_add(line.len())
                .ok_or_else(too_large)?;
            if self.event_bytes > MAX_SSE_EVENT_BYTES {
                return Err(too_large());
            }
            let (field, value) = line.split_once(':').map_or((line, ""), |(field, value)| {
                (field, value.strip_prefix(' ').unwrap_or(value))
            });
            match field {
                "event" => {
                    if value.is_empty() || self.event_name.replace(value.to_owned()).is_some() {
                        return Err(malformed());
                    }
                }
                "data" => self.data_lines.push(value.to_owned()),
                "id" => {
                    if value.contains('\0') {
                        return Err(malformed());
                    }
                }
                "retry" => {
                    if value.parse::<u64>().is_err() {
                        return Err(malformed());
                    }
                }
                _ => return Err(malformed()),
            }
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Result<(), ProviderError> {
        if self.data_lines.is_empty() {
            return Err(malformed());
        }
        self.event_count = self.event_count.checked_add(1).ok_or_else(too_large)?;
        if self.event_count > MAX_SSE_EVENTS {
            return Err(too_large());
        }
        self.ready.push_back(SseEvent {
            event: self.event_name.take(),
            data: std::mem::take(&mut self.data_lines).join("\n"),
        });
        self.event_bytes = 0;
        Ok(())
    }

    pub(super) fn pop_event(&mut self) -> Option<SseEvent> {
        self.ready.pop_front()
    }

    pub(super) fn has_event(&self) -> bool {
        !self.ready.is_empty()
    }
}

fn malformed() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::MalformedResponse,
        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
    )
}

fn too_large() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::OutputTooLarge,
        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
    )
}
