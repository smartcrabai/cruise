use serde::Serialize;

/// Events streamed during session creation (plan generation) over an IPC Channel.
///
/// Uses the same adjacently-tagged format as [`WorkflowEvent`]:
/// `{ "event": "<variantName>", "data": { ... } }`
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum PlanEvent {
    /// Session has been persisted to disk; emitted before plan generation begins so the
    /// frontend can release the New Session form without waiting for the plan.
    SessionCreated {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// Plan generation has started.
    PlanGenerating,
    /// Plan was generated successfully; `content` is the markdown text.
    /// `session_id` identifies which session this plan belongs to.
    PlanGenerated {
        #[serde(rename = "sessionId")]
        session_id: String,
        content: String,
    },
    /// Plan generation failed; `error` contains the error message.
    /// `session_id` identifies which session failed.
    PlanFailed {
        #[serde(rename = "sessionId")]
        session_id: String,
        error: String,
    },
}

/// A single choice in an option step, serialized for IPC transport to the frontend.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceDto {
    pub label: String,
    pub kind: ChoiceKind,
    pub next: Option<String>,
}

/// Discriminates between selector and free-text-input choices.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ChoiceKind {
    Selector,
    TextInput,
}

/// Events streamed from the Tauri backend to the frontend over an IPC Channel.
///
/// Serialized with an adjacently-tagged format:
/// `{ "event": "<variantName>", "data": { ... } }`
///
/// Variant names and field names are converted to camelCase.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum WorkflowEvent {
    StepStarted {
        /// The session this event belongs to.
        /// Required for attributing concurrent progress to the correct session.
        #[serde(rename = "sessionId")]
        session_id: String,
        step: String,
    },
    StepCompleted {
        step: String,
        success: bool,
        #[serde(rename = "durationMs")]
        duration_ms: u64,
        output: Option<String>,
    },
    OptionRequired {
        #[serde(rename = "requestId")]
        request_id: String,
        choices: Vec<ChoiceDto>,
        plan: Option<String>,
    },
    WorkflowCompleted {
        /// The session this event belongs to.
        #[serde(rename = "sessionId")]
        session_id: String,
        run: usize,
        skipped: usize,
        failed: usize,
    },
    WorkflowFailed {
        /// The session this event belongs to.
        #[serde(rename = "sessionId")]
        session_id: String,
        error: String,
    },
    WorkflowCancelled {
        /// The session this event belongs to.
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    RunAllStarted {
        total: usize,
        /// Configured parallelism for this batch (for display / debugging).
        parallelism: usize,
    },
    RunAllSessionStarted {
        #[serde(rename = "sessionId")]
        session_id: String,
        input: String,
        total: usize,
    },
    RunAllSessionFinished {
        #[serde(rename = "sessionId")]
        session_id: String,
        input: String,
        phase: String,
        error: Option<String>,
    },
    RunAllCompleted {
        cancelled: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn to_json<T: serde::Serialize>(event: &T) -> Value {
        serde_json::to_value(event).unwrap_or_else(|e| panic!("serialization failed: {e}"))
    }

    // --- PlanEvent ------------------------------------------------------------

    #[test]
    fn plan_event_session_created_serializes_session_id_as_camel_case() {
        // Given: a SessionCreated event
        let event = PlanEvent::SessionCreated {
            session_id: "sess-42".to_string(),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: event tag is "sessionCreated" and data.sessionId is correct
        assert_eq!(json["event"], "sessionCreated");
        assert_eq!(json["data"]["sessionId"], "sess-42");
    }

    #[test]
    fn plan_event_plan_generating_serializes_event_tag_only() {
        // Given: a PlanGenerating event (no payload)
        let event = PlanEvent::PlanGenerating;
        // When: serialized
        let json = to_json(&event);
        // Then: event tag is present; adjacently-tagged unit variants omit data
        assert_eq!(json["event"], "planGenerating");
        assert!(json.get("data").is_none() || json["data"] == Value::Null);
    }

    #[test]
    fn plan_event_plan_generated_serializes_all_fields() {
        let event = PlanEvent::PlanGenerated {
            session_id: "sess-1".to_string(),
            content: "# Plan\n- step 1".to_string(),
        };
        let json = to_json(&event);
        assert_eq!(json["event"], "planGenerated");
        assert_eq!(json["data"]["sessionId"], "sess-1");
        assert_eq!(json["data"]["content"], "# Plan\n- step 1");
    }

    #[test]
    fn plan_event_plan_failed_serializes_all_fields() {
        let event = PlanEvent::PlanFailed {
            session_id: "sess-2".to_string(),
            error: "model error".to_string(),
        };
        let json = to_json(&event);
        assert_eq!(json["event"], "planFailed");
        assert_eq!(json["data"]["sessionId"], "sess-2");
        assert_eq!(json["data"]["error"], "model error");
    }

    // --- StepStarted ---

    #[test]
    fn test_step_started_serializes_step_name_and_session_id() {
        // Given: a StepStarted event with session context
        let event = WorkflowEvent::StepStarted {
            session_id: "sess-42".to_string(),
            step: "build".to_string(),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: sessionId and step are both present under camelCase keys
        assert_eq!(json["event"], "stepStarted");
        assert_eq!(json["data"]["sessionId"], "sess-42");
        assert_eq!(json["data"]["step"], "build");
    }

    // --- StepCompleted ---

    #[test]
    fn test_step_completed_serializes_all_fields_as_camel_case() {
        // Given: a StepCompleted event with output
        let event = WorkflowEvent::StepCompleted {
            step: "test".to_string(),
            success: true,
            duration_ms: 123,
            output: Some("ok".to_string()),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: field names use camelCase and all values are present
        assert_eq!(json["event"], "stepCompleted");
        assert_eq!(json["data"]["step"], "test");
        assert_eq!(json["data"]["success"], true);
        assert_eq!(json["data"]["durationMs"], 123);
        assert_eq!(json["data"]["output"], "ok");
    }

    #[test]
    fn test_step_completed_with_no_output_serializes_as_null() {
        // Given: a StepCompleted event without output
        let event = WorkflowEvent::StepCompleted {
            step: "noop".to_string(),
            success: false,
            duration_ms: 0,
            output: None,
        };
        // When: serialized
        let json = to_json(&event);
        // Then: output field is null
        assert_eq!(json["data"]["output"], Value::Null);
    }

    // --- OptionRequired ---

    #[test]
    fn test_option_required_serializes_request_id_and_selector_choice() {
        // Given: an OptionRequired event with one selector choice
        let event = WorkflowEvent::OptionRequired {
            request_id: "req-1".to_string(),
            choices: vec![ChoiceDto {
                label: "Continue".to_string(),
                kind: ChoiceKind::Selector,
                next: Some("next_step".to_string()),
            }],
            plan: None,
        };
        // When: serialized
        let json = to_json(&event);
        // Then: tag, requestId, and choice fields are correct
        assert_eq!(json["event"], "optionRequired");
        assert_eq!(json["data"]["requestId"], "req-1");
        assert_eq!(json["data"]["choices"][0]["label"], "Continue");
        assert_eq!(json["data"]["choices"][0]["kind"], "selector");
        assert_eq!(json["data"]["choices"][0]["next"], "next_step");
        assert_eq!(json["data"]["plan"], Value::Null);
    }

    #[test]
    fn test_option_required_with_plan_includes_plan_text() {
        // Given: an OptionRequired event that includes plan context
        let event = WorkflowEvent::OptionRequired {
            request_id: "req-2".to_string(),
            choices: vec![],
            plan: Some("## Plan\nStep 1: do X".to_string()),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: plan field is present
        assert_eq!(json["data"]["plan"], "## Plan\nStep 1: do X");
    }

    // --- WorkflowCompleted ---

    #[test]
    fn test_workflow_completed_serializes_session_id_and_counts() {
        // Given: a WorkflowCompleted event with session context
        let event = WorkflowEvent::WorkflowCompleted {
            session_id: "sess-1".to_string(),
            run: 3,
            skipped: 1,
            failed: 0,
        };
        // When: serialized
        let json = to_json(&event);
        // Then: sessionId and all counts are present
        assert_eq!(json["event"], "workflowCompleted");
        assert_eq!(json["data"]["sessionId"], "sess-1");
        assert_eq!(json["data"]["run"], 3);
        assert_eq!(json["data"]["skipped"], 1);
        assert_eq!(json["data"]["failed"], 0);
    }

    // --- WorkflowFailed ---

    #[test]
    fn test_workflow_failed_serializes_session_id_and_error_message() {
        // Given: a WorkflowFailed event with session context
        let event = WorkflowEvent::WorkflowFailed {
            session_id: "sess-2".to_string(),
            error: "step 'build' failed".to_string(),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: sessionId and error message are both present
        assert_eq!(json["event"], "workflowFailed");
        assert_eq!(json["data"]["sessionId"], "sess-2");
        assert_eq!(json["data"]["error"], "step 'build' failed");
    }

    // --- WorkflowCancelled ---

    #[test]
    fn test_workflow_cancelled_serializes_session_id() {
        // Given: a WorkflowCancelled event with session context
        let event = WorkflowEvent::WorkflowCancelled {
            session_id: "sess-3".to_string(),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: sessionId is present (WorkflowCancelled is no longer a unit variant)
        assert_eq!(json["event"], "workflowCancelled");
        assert_eq!(json["data"]["sessionId"], "sess-3");
    }

    // --- ChoiceKind ---

    // --- RunAllStarted ---

    #[test]
    fn test_run_all_started_serializes_total_and_parallelism() {
        // Given: a RunAllStarted event with 3 candidate sessions and parallelism=2
        let event = WorkflowEvent::RunAllStarted {
            total: 3,
            parallelism: 2,
        };
        // When: serialized to JSON
        let json = to_json(&event);
        // Then: event tag is camelCase; both total and parallelism are present
        assert_eq!(json["event"], "runAllStarted");
        assert_eq!(json["data"]["total"], 3);
        assert_eq!(json["data"]["parallelism"], 2);
    }

    #[test]
    fn test_run_all_started_parallelism_one_is_backward_compatible_default() {
        // Given: a RunAllStarted event with parallelism=1 (sequential default)
        let event = WorkflowEvent::RunAllStarted {
            total: 5,
            parallelism: 1,
        };
        // When: serialized
        let json = to_json(&event);
        // Then: parallelism field is present and equals 1
        assert_eq!(json["data"]["parallelism"], 1);
    }

    // --- RunAllSessionStarted ---

    #[test]
    fn test_run_all_session_started_serializes_all_fields_as_camel_case() {
        // Given: a RunAllSessionStarted event with all fields including total
        let event = WorkflowEvent::RunAllSessionStarted {
            session_id: "sess-1".to_string(),
            input: "do something".to_string(),
            total: 3,
        };
        // When: serialized to JSON
        let json = to_json(&event);
        // Then: all fields are present with camelCase names
        assert_eq!(json["event"], "runAllSessionStarted");
        assert_eq!(json["data"]["sessionId"], "sess-1");
        assert_eq!(json["data"]["input"], "do something");
        assert_eq!(json["data"]["total"], 3);
    }

    // --- RunAllSessionFinished ---

    #[test]
    fn test_run_all_session_finished_without_error_serializes_null_error() {
        // Given: a RunAllSessionFinished event with no error (session completed)
        let event = WorkflowEvent::RunAllSessionFinished {
            session_id: "sess-1".to_string(),
            input: "do something".to_string(),
            phase: "Completed".to_string(),
            error: None,
        };
        // When: serialized
        let json = to_json(&event);
        // Then: error field is null and phase/sessionId are present
        assert_eq!(json["event"], "runAllSessionFinished");
        assert_eq!(json["data"]["sessionId"], "sess-1");
        assert_eq!(json["data"]["input"], "do something");
        assert_eq!(json["data"]["phase"], "Completed");
        assert_eq!(json["data"]["error"], Value::Null);
    }

    #[test]
    fn test_run_all_session_finished_with_error_includes_error_message() {
        // Given: a RunAllSessionFinished event where the session failed
        let event = WorkflowEvent::RunAllSessionFinished {
            session_id: "sess-2".to_string(),
            input: "build project".to_string(),
            phase: "Failed".to_string(),
            error: Some("step 'build' failed".to_string()),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: error message is present alongside the phase
        assert_eq!(json["event"], "runAllSessionFinished");
        assert_eq!(json["data"]["sessionId"], "sess-2");
        assert_eq!(json["data"]["input"], "build project");
        assert_eq!(json["data"]["phase"], "Failed");
        assert_eq!(json["data"]["error"], "step 'build' failed");
    }

    // --- RunAllCompleted ---

    #[test]
    fn test_run_all_completed_serializes_cancelled_count() {
        // Given: a RunAllCompleted event where one session was cancelled
        let event = WorkflowEvent::RunAllCompleted { cancelled: 1 };
        // When: serialized
        let json = to_json(&event);
        // Then: cancelled count is present under the correct tag
        assert_eq!(json["event"], "runAllCompleted");
        assert_eq!(json["data"]["cancelled"], 1);
    }

    #[test]
    fn test_choice_kind_text_input_serializes_as_camel_case() {
        // Given: a TextInput choice DTO
        let choice = ChoiceDto {
            label: "Enter name".to_string(),
            kind: ChoiceKind::TextInput,
            next: None,
        };
        // When: serialized
        let json = serde_json::to_value(&choice).unwrap_or_else(|e| panic!("{e}"));
        // Then: kind is "textInput" (camelCase)
        assert_eq!(json["kind"], "textInput");
        assert_eq!(json["next"], Value::Null);
    }
}
