use super::types::{Error, Event, Result};
use indexmap::IndexMap;

/// Byte-oriented framing avoids corrupting UTF-8 split across network chunks.
#[derive(Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
    data: Vec<String>,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Event>> {
        if self.pending.len().saturating_add(bytes.len()) > 2 * 1024 * 1024 {
            return Err(Error::Protocol("SSE frame exceeds limit".into()));
        }
        self.pending.extend_from_slice(bytes);
        let mut events = vec![];
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = std::str::from_utf8(&line)
                .map_err(|e| Error::Protocol(e.to_string()))?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    let value = self.data.join("\n");
                    self.data.clear();
                    let event = serde_json::from_str(&value)
                        .map_err(|e| Error::Protocol(format!("malformed SSE JSON: {e}")))?;
                    events.push(event);
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
                if self.data.iter().map(String::len).sum::<usize>() > 2 * 1024 * 1024 {
                    return Err(Error::Protocol("SSE data exceeds limit".into()));
                }
            }
        }
        Ok(events)
    }
}

#[derive(Default)]
pub(crate) struct Turn {
    pub(crate) usage_rounds: Vec<crate::usage::UsageRound>,
    usage_seen: std::collections::HashSet<String>,
    pub sequence: u64,
    pub started: bool,
    pub delivered: bool,
    pub terminal: Option<std::result::Result<(), String>>,
    pub failure: Option<super::types::ModelFailure>,
    output_bytes: usize,
    text: IndexMap<(String, u64), String>,
    ended: std::collections::HashSet<(String, u64)>,
}
impl Turn {
    pub fn output(&self) -> String {
        self.text.values().map(String::as_str).collect()
    }
    #[expect(
        clippy::too_many_lines,
        reason = "Ordered durable turn reduction keeps validation, deduplication and terminal transitions together"
    )]
    pub fn apply(&mut self, event: Event, session: &str, inbox: &str) -> Result<Option<String>> {
        let mandatory = matches!(
            event.kind.as_str(),
            "session.execution.started"
                | "session.execution.succeeded"
                | "session.execution.failed"
                | "session.execution.interrupted"
                | "session.inbox.delivered"
                | "session.text.started"
                | "session.text.delta"
                | "session.text.ended"
        );
        if mandatory && event.data["sessionID"].as_str().is_none() {
            return Err(Error::Protocol(format!("{} missing sessionID", event.kind)));
        }
        if event.data["sessionID"].as_str() != Some(session) {
            return Ok(None);
        }
        if let Some(durable) = event.durable {
            if durable.aggregate_id != session {
                return Err(Error::Protocol(
                    "durable event aggregate does not match session".into(),
                ));
            }
            if durable.seq <= self.sequence {
                return Ok(None);
            }
            // Public V2 events omit internal durable records such as title usage.
            // Their aggregate sequence is monotonic, not necessarily contiguous.
            self.sequence = durable.seq;
        }
        match event.kind.as_str() {
            "session.step.ended" | "session.step.failed" if self.started && self.delivered => {
                if let (Some(tokens), Some(id)) = (
                    event.data.get("tokens").filter(|v| v.is_object()),
                    event.data["assistantMessageID"].as_str(),
                ) {
                    if self.usage_seen.insert(id.to_owned()) {
                        self.usage_rounds.push(crate::usage::UsageRound {
                            model: None,
                            counts: crate::usage::UsageCounts {
                                input_tokens: tokens["input"].as_u64(),
                                output_tokens: tokens["output"].as_u64(),
                                reasoning_tokens: tokens["reasoning"].as_u64(),
                                cache_read_input_tokens: tokens["cache"]["read"].as_u64(),
                                cache_creation_input_tokens: tokens["cache"]["write"].as_u64(),
                                cost_usd: event.data["cost"].as_f64(),
                            },
                        });
                    }
                }
            }
            "session.execution.started" => {
                self.started = true;
            }
            "session.inbox.delivered" => {
                let delivered = event.data["inboxID"]
                    .as_str()
                    .ok_or_else(|| Error::Protocol("inbox delivery missing inboxID".into()))?;
                if delivered == inbox {
                    self.delivered = true;
                }
            }
            "session.text.delta" | "session.text.ended" if self.started && self.delivered => {
                let id = event.data["assistantMessageID"]
                    .as_str()
                    .ok_or_else(|| Error::Protocol("text missing assistantMessageID".into()))?;
                let ordinal = event.data["ordinal"]
                    .as_u64()
                    .ok_or_else(|| Error::Protocol("text missing ordinal".into()))?;
                let full = event.kind == "session.text.ended";
                let value = event.data[if full { "text" } else { "delta" }]
                    .as_str()
                    .ok_or_else(|| Error::Protocol("text missing content".into()))?;
                let key = (id.to_owned(), ordinal);
                if self.ended.contains(&key) {
                    return Ok(None);
                }
                let entry = self.text.entry(key).or_default();
                if full {
                    self.ended.insert((id.to_owned(), ordinal));
                    if let Some(suffix) = value.strip_prefix(entry.as_str()) {
                        let suffix = suffix.to_owned();
                        if self.output_bytes.saturating_add(suffix.len()) > 64 * 1024 * 1024 {
                            return Err(Error::Protocol("assistant output exceeds limit".into()));
                        }
                        self.output_bytes += suffix.len();
                        entry.push_str(&suffix);
                        return Ok((!suffix.is_empty()).then_some(suffix));
                    }
                    return Err(Error::Protocol(
                        "durable text disagrees with streamed prefix".into(),
                    ));
                }
                if self.output_bytes.saturating_add(value.len()) > 64 * 1024 * 1024 {
                    return Err(Error::Protocol("assistant output exceeds limit".into()));
                }
                self.output_bytes += value.len();
                entry.push_str(value);
                return Ok(Some(value.to_owned()));
            }
            "session.execution.succeeded" if self.started && self.delivered => {
                self.terminal = Some(Ok(()));
            }
            "session.execution.failed" if self.started => {
                let failure: super::types::ModelFailure =
                    serde_json::from_value(event.data["error"].clone())
                        .map_err(|e| Error::Protocol(format!("invalid execution failure: {e}")))?;
                if failure
                    .status
                    .is_some_and(|status| !(100..=599).contains(&status))
                {
                    return Err(Error::Protocol(
                        "execution failure has invalid HTTP status".into(),
                    ));
                }
                self.terminal = Some(Err(failure.to_string()));
                self.failure = Some(failure);
            }
            "session.execution.interrupted" if self.started => {
                self.terminal = Some(Err("OpenCode execution interrupted".into()));
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use serde_json::{Value, json};

    fn step(kind: &str, fields: Value) -> Event {
        let mut data = json!({"sessionID":"s", "assistantMessageID":"m"});
        data.as_object_mut()
            .unwrap_or_else(|| panic!("fixture data must be an object"))
            .extend(
                fields
                    .as_object()
                    .unwrap_or_else(|| panic!("fixture fields must be an object"))
                    .clone(),
            );
        serde_json::from_value(json!({"type":kind,"data":data}))
            .unwrap_or_else(|error| panic!("invalid event fixture: {error}"))
    }

    #[test]
    fn opencode_step_ended_counted_only_after_inbox_delivery() {
        let mut turn = Turn::default();
        turn.apply(
            step("session.step.ended", json!({"tokens":{"input":5}})),
            "s",
            "i",
        )
        .unwrap_or_else(|error| panic!("event application failed: {error}"));
        assert!(turn.usage_rounds.is_empty());
        turn.started = true;
        turn.delivered = true;
        turn.apply(
            step("session.step.ended", json!({"tokens":{"input":5}})),
            "s",
            "i",
        )
        .unwrap_or_else(|error| panic!("event application failed: {error}"));
        assert_eq!(turn.usage_rounds.len(), 1);
    }

    #[test]
    fn opencode_step_events_deduplicate_by_assistant_message_id() {
        let mut turn = Turn {
            started: true,
            delivered: true,
            ..Default::default()
        };
        for _ in 0..2 {
            turn.apply(
                step("session.step.ended", json!({"tokens":{"input":5}})),
                "s",
                "i",
            )
            .unwrap_or_else(|error| panic!("event application failed: {error}"));
        }
        assert_eq!(turn.usage_rounds.len(), 1);
    }

    #[test]
    fn opencode_step_maps_tokens_reasoning_cache_and_cost() {
        let mut turn = Turn {
            started: true,
            delivered: true,
            ..Default::default()
        };
        turn.apply(step("session.step.ended", json!({"tokens":{"input":5,"output":6,"reasoning":7,"cache":{"read":8,"write":9}},"cost":0.25})), "s", "i").unwrap_or_else(|error| panic!("event application failed: {error}"));
        let round = &turn.usage_rounds[0];
        assert_eq!(round.model, None);
        assert_eq!(round.counts.input_tokens, Some(5));
        assert_eq!(round.counts.output_tokens, Some(6));
        assert_eq!(round.counts.reasoning_tokens, Some(7));
        assert_eq!(round.counts.cache_read_input_tokens, Some(8));
        assert_eq!(round.counts.cache_creation_input_tokens, Some(9));
        assert_eq!(round.counts.cost_usd, Some(0.25));
    }

    #[test]
    fn opencode_step_failed_without_tokens_contributes_nothing() {
        let mut turn = Turn {
            started: true,
            delivered: true,
            ..Default::default()
        };
        turn.apply(step("session.step.failed", json!({"cost":0.25})), "s", "i")
            .unwrap_or_else(|error| panic!("event application failed: {error}"));
        assert!(turn.usage_rounds.is_empty());
    }
}
