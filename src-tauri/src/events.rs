use serde::Serialize;

/// Events streamed during session creation (plan generation) over an IPC Channel.
///
/// Uses the same adjacently-tagged format as [`WorkflowEvent`]:
/// `{ "event": "<variantName>", "data": { ... } }`
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum PlanEvent {
    /// Plan generation has started.
    PlanGenerating,
    /// Plan was generated successfully; `content` is the markdown text.
    PlanGenerated { content: String },
    /// Plan generation failed; `error` contains the error message.
    PlanFailed { error: String },
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
        run: usize,
        skipped: usize,
        failed: usize,
    },
    WorkflowFailed {
        error: String,
    },
    WorkflowCancelled,
    RunAllStarted {
        total: usize,
    },
    RunAllSessionStarted {
        #[serde(rename = "sessionId")]
        session_id: String,
        input: String,
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

    fn to_json(event: &WorkflowEvent) -> Value {
        serde_json::to_value(event).unwrap_or_else(|e| panic!("serialization failed: {e}"))
    }

    // --- StepStarted ---

    #[test]
    fn test_step_started_serializes_step_name_only() {
        let event = WorkflowEvent::StepStarted {
            step: "build".to_string(),
        };
        let json = to_json(&event);
        assert_eq!(json["event"], "stepStarted");
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
    fn test_workflow_completed_serializes_run_skipped_failed_counts() {
        // Given: a WorkflowCompleted event
        let event = WorkflowEvent::WorkflowCompleted {
            run: 3,
            skipped: 1,
            failed: 0,
        };
        // When: serialized
        let json = to_json(&event);
        // Then: counts are present
        assert_eq!(json["event"], "workflowCompleted");
        assert_eq!(json["data"]["run"], 3);
        assert_eq!(json["data"]["skipped"], 1);
        assert_eq!(json["data"]["failed"], 0);
    }

    // --- WorkflowFailed ---

    #[test]
    fn test_workflow_failed_serializes_with_error_message() {
        // Given: a WorkflowFailed event
        let event = WorkflowEvent::WorkflowFailed {
            error: "step 'build' failed".to_string(),
        };
        // When: serialized
        let json = to_json(&event);
        // Then: error message is present under the correct tag
        assert_eq!(json["event"], "workflowFailed");
        assert_eq!(json["data"]["error"], "step 'build' failed");
    }

    // --- WorkflowCancelled ---

    #[test]
    fn test_workflow_cancelled_serializes_event_tag_without_data() {
        // Given: a WorkflowCancelled unit variant
        let event = WorkflowEvent::WorkflowCancelled;
        // When: serialized
        let json = to_json(&event);
        // Then: only the event tag is present; adjacently-tagged unit variants omit data
        assert_eq!(json["event"], "workflowCancelled");
        assert!(json.get("data").is_none() || json["data"] == Value::Null);
    }

    // --- ChoiceKind ---

    // --- RunAllStarted ---

    #[test]
    fn test_run_all_started_serializes_total() {
        // Given: a RunAllStarted event with 3 candidate sessions
        let event = WorkflowEvent::RunAllStarted { total: 3 };
        // When: serialized to JSON
        let json = to_json(&event);
        // Then: event tag is camelCase and total is correct
        assert_eq!(json["event"], "runAllStarted");
        assert_eq!(json["data"]["total"], 3);
    }

    // --- RunAllSessionStarted ---

    #[test]
    fn test_run_all_session_started_serializes_all_fields_as_camel_case() {
        // Given: a RunAllSessionStarted event
        let event = WorkflowEvent::RunAllSessionStarted {
            session_id: "sess-1".to_string(),
            input: "do something".to_string(),
        };
        // When: serialized to JSON
        let json = to_json(&event);
        // Then: all fields are present with camelCase names
        assert_eq!(json["event"], "runAllSessionStarted");
        assert_eq!(json["data"]["sessionId"], "sess-1");
        assert_eq!(json["data"]["input"], "do something");
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
