//! What a backend emits while a prompt runs, and how cruise folds it.
//!
//! A backend produces [`StreamChunk`]s on a channel; [`ChunkReducer`] folds them
//! into a terminal [`ChunkOutcome`] while [`LineBuffer`] reshapes token-level
//! deltas into the whole lines a `command`-style stdout callback expects.

/// A provider rate/usage limit reported by a backend, distinguished from an
/// ordinary error because it is retryable.
#[derive(Debug, thiserror::Error)]
#[error("Provider '{provider}' hit API rate/usage limit")]
pub struct LimitError {
    pub provider: String,
}

/// One event from a running prompt.
#[derive(Debug)]
pub enum StreamChunk {
    /// Streaming text delta from the assistant.
    Delta(String),
    /// Final completion with the full assistant text (may be empty if all text
    /// was already surfaced via `Delta`).
    Done(String),
    /// The backend session id for this run (newly generated or resumed).
    /// Emitted once, before the terminal chunk, so consumers can persist it for
    /// a follow-up turn -- but not necessarily before the first `Delta`: a
    /// backend that only learns the id from its turn-completion event reports it
    /// after the text.
    Session(String),
    /// The provider reported a rate/usage limit.
    Limit(LimitError),
    /// Any other error (provider error, transport error, ...) -- stringified.
    Error(String),
}

/// Buffers streamed token fragments and emits them as complete lines, so a
/// line-oriented [`crate::step::prompt::StreamCallbacks::on_stdout`] sees the
/// same shape from an SDK backend as from the command backend.
pub(crate) struct LineBuffer {
    pending: String,
}

impl LineBuffer {
    pub(crate) fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    /// Append `frag`, emitting each newly-completed line (without its trailing
    /// `\n`/`\r\n`).
    pub(crate) fn push<F: FnMut(&str)>(&mut self, frag: &str, mut emit: F) {
        self.pending.push_str(frag);
        while let Some(idx) = self.pending.find('\n') {
            let rest = self.pending.split_off(idx + 1);
            let mut line = std::mem::replace(&mut self.pending, rest);
            line.pop(); // drop '\n'
            if line.ends_with('\r') {
                line.pop();
            }
            emit(&line);
        }
    }

    /// Emit any buffered partial line (no trailing newline) and clear.
    pub(crate) fn flush<F: FnMut(&str)>(&mut self, mut emit: F) {
        if !self.pending.is_empty() {
            emit(&self.pending);
            self.pending.clear();
        }
    }
}

/// Terminal (or closed) outcome of folding a stream of [`StreamChunk`]s.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChunkOutcome {
    /// The run completed with the given full text.
    Done {
        output: String,
        session: Option<String>,
    },
    /// The provider reported a rate/usage limit.
    Limited {
        message: String,
        session: Option<String>,
    },
    /// The run failed with a non-limit error.
    Failed {
        message: String,
        session: Option<String>,
    },
    /// The channel closed before any terminal chunk arrived.
    Closed {
        partial: String,
        session: Option<String>,
    },
}

/// Incrementally folds [`StreamChunk`]s into a [`ChunkOutcome`], surfacing text
/// deltas through an `on_delta` sink as they arrive.
pub(crate) struct ChunkReducer {
    buf: String,
    session: Option<String>,
}

impl ChunkReducer {
    pub(crate) fn new() -> Self {
        Self {
            buf: String::new(),
            session: None,
        }
    }

    /// Feed one chunk. Returns `Some(outcome)` when a terminal chunk arrives,
    /// `None` to keep consuming.
    pub(crate) fn step<F: FnMut(&str)>(
        &mut self,
        chunk: StreamChunk,
        on_delta: &mut F,
    ) -> Option<ChunkOutcome> {
        match chunk {
            StreamChunk::Delta(d) => {
                on_delta(&d);
                self.buf.push_str(&d);
                None
            }
            StreamChunk::Session(id) => {
                self.session = Some(id);
                None
            }
            StreamChunk::Done(text) => {
                let output = if text.is_empty() {
                    std::mem::take(&mut self.buf)
                } else {
                    text
                };
                Some(ChunkOutcome::Done {
                    output,
                    session: self.session.take(),
                })
            }
            StreamChunk::Limit(e) => Some(ChunkOutcome::Limited {
                message: e.to_string(),
                session: self.session.take(),
            }),
            StreamChunk::Error(msg) => Some(ChunkOutcome::Failed {
                message: msg,
                session: self.session.take(),
            }),
        }
    }

    /// Produce a [`ChunkOutcome::Closed`] when the stream ends without a terminal
    /// chunk.
    pub(crate) fn finish(&mut self) -> ChunkOutcome {
        ChunkOutcome::Closed {
            partial: std::mem::take(&mut self.buf),
            session: self.session.take(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- ChunkReducer ---------------------------------------------------------

    fn no_sink() -> impl FnMut(&str) {
        |_: &str| {}
    }

    #[test]
    fn reducer_accumulates_deltas_and_captures_session() {
        let mut r = ChunkReducer::new();
        let mut collected = String::new();
        let mut sink = |d: &str| collected.push_str(d);
        assert_eq!(
            r.step(StreamChunk::Session("sid-1".to_string()), &mut sink),
            None
        );
        assert_eq!(
            r.step(StreamChunk::Delta("Hello ".to_string()), &mut sink),
            None
        );
        assert_eq!(
            r.step(StreamChunk::Delta("world".to_string()), &mut sink),
            None
        );
        let out = r
            .step(StreamChunk::Done(String::new()), &mut sink)
            .unwrap_or_else(|| panic!("expected terminal"));
        assert_eq!(collected, "Hello world");
        assert_eq!(
            out,
            ChunkOutcome::Done {
                output: "Hello world".to_string(),
                session: Some("sid-1".to_string()),
            }
        );
    }

    #[test]
    fn reducer_done_text_overrides_buffered_deltas() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        r.step(StreamChunk::Delta("partial".to_string()), &mut sink);
        let out = r
            .step(StreamChunk::Done("FINAL".to_string()), &mut sink)
            .unwrap_or_else(|| panic!("expected terminal"));
        assert_eq!(
            out,
            ChunkOutcome::Done {
                output: "FINAL".to_string(),
                session: None,
            }
        );
    }

    #[test]
    fn reducer_surfaces_error_chunk() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        let out = r
            .step(StreamChunk::Error("boom".to_string()), &mut sink)
            .unwrap_or_else(|| panic!("expected terminal"));
        assert_eq!(
            out,
            ChunkOutcome::Failed {
                message: "boom".to_string(),
                session: None,
            }
        );
    }

    #[test]
    fn reducer_surfaces_limit_chunk() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        let out = r
            .step(
                StreamChunk::Limit(LimitError {
                    provider: "anthropic".to_string(),
                }),
                &mut sink,
            )
            .unwrap_or_else(|| panic!("expected terminal"));
        match out {
            ChunkOutcome::Limited { message, .. } => {
                assert!(message.contains("anthropic"), "got: {message}");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // -- LineBuffer -----------------------------------------------------------

    fn collect_lines(frags: &[&str]) -> (Vec<String>, Vec<String>) {
        let mut lb = LineBuffer::new();
        let mut lines = Vec::new();
        for f in frags {
            lb.push(f, |l| lines.push(l.to_string()));
        }
        let mut flushed = Vec::new();
        lb.flush(|l| flushed.push(l.to_string()));
        (lines, flushed)
    }

    #[test]
    fn line_buffer_emits_complete_lines_and_flushes_remainder() {
        let (lines, flushed) = collect_lines(&["Hel", "lo\nwor", "ld"]);
        assert_eq!(lines, vec!["Hello".to_string()]);
        assert_eq!(flushed, vec!["world".to_string()]);
    }

    #[test]
    fn line_buffer_handles_multiple_lines_in_one_fragment() {
        let (lines, flushed) = collect_lines(&["a\nb\nc\n"]);
        assert_eq!(
            lines,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(flushed.is_empty(), "no partial line should remain");
    }

    #[test]
    fn line_buffer_strips_carriage_return() {
        let (lines, _) = collect_lines(&["x\r\n"]);
        assert_eq!(lines, vec!["x".to_string()]);
    }

    #[test]
    fn reducer_finish_reports_closed_with_partial() {
        let mut r = ChunkReducer::new();
        let mut sink = no_sink();
        r.step(StreamChunk::Delta("half".to_string()), &mut sink);
        assert_eq!(
            r.finish(),
            ChunkOutcome::Closed {
                partial: "half".to_string(),
                session: None,
            }
        );
    }
}
