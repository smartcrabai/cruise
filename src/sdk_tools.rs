//! Custom seher tools (function calling) for interactive planning.
//!
//! Three tools are exposed to the planning agent:
//! - `ask_user(question)` — ask the user a clarifying question and return their
//!   answer (delegates to an [`AskHandler`]).
//! - `submit_plan(content)` — write the full plan markdown to the session
//!   `plan.md`.
//! - `update_plan(old, new)` — find/replace a section of the existing `plan.md`.
//!
//! Tool handlers are synchronous `Arc` closures (`'static`), so the plan path is
//! captured by value and the [`AskHandler`] is shared via `Arc`. A handler that
//! returns `Err(msg)` surfaces to the model with `is_error: true` so it can
//! recover (e.g. re-read the plan and retry an `update_plan` whose `old` text no
//! longer matches) rather than aborting the turn.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use seher::sdk::{SeherTool, ToolHandler};
use serde_json::json;

use crate::ask_handler::AskHandler;

/// Tool name for the clarifying-question tool.
pub const ASK_USER_TOOL: &str = "ask_user";
/// Tool name for the full-plan submission tool.
pub const SUBMIT_PLAN_TOOL: &str = "submit_plan";
/// Tool name for the section find/replace tool.
pub const UPDATE_PLAN_TOOL: &str = "update_plan";
/// Tool name for the session title generation tool.
pub const GENERATE_TITLE_TOOL: &str = "generate_title";
/// Tool name for the PR metadata submission tool.
pub const SUBMIT_PR_METADATA_TOOL: &str = "submit_pr_metadata";

/// Shared flag recording whether the planning agent persisted the plan during
/// the current turn. Set to `true` only by a *successful* `submit_plan` /
/// `update_plan` call; a handler error (e.g. a stale `update_plan` snippet or a
/// failed write) leaves it untouched so the agent can retry.
pub type PlanPersistFlag = Arc<AtomicBool>;

/// The planning tool vec plus the shared persist flag for the turn. The caller
/// inspects [`PlanningToolSet::plan_persisted`] after the turn ends to verify
/// the agent actually persisted the plan instead of just talking about one.
pub struct PlanningToolSet {
    /// Tools to register with the SDK backend.
    pub tools: Vec<SeherTool>,
    /// Set once the plan has been persisted via `submit_plan` / `update_plan`.
    pub plan_persisted: PlanPersistFlag,
}

/// Build the planning tool set.
///
/// `interactive` controls whether the user can be reached:
/// - `true`  -> `[ask_user, submit_plan, update_plan]` (the agent can ask
///   questions and iteratively revise the plan).
/// - `false` -> `[submit_plan, update_plan]` (non-TTY runs: no `ask_user`,
///   since no one can answer; `update_plan` needs no user interaction and the
///   fix-plan template relies on it for targeted edits).
#[must_use]
pub fn planning_tools(
    plan_path: PathBuf,
    ask: Arc<dyn AskHandler>,
    interactive: bool,
) -> PlanningToolSet {
    let plan_persisted: PlanPersistFlag = Arc::new(AtomicBool::new(false));
    let mut tools = Vec::new();
    if interactive {
        tools.push(ask_user_tool(ask));
    }
    tools.push(submit_plan_tool(
        plan_path.clone(),
        Arc::clone(&plan_persisted),
    ));
    tools.push(update_plan_tool(plan_path, Arc::clone(&plan_persisted)));
    PlanningToolSet {
        tools,
        plan_persisted,
    }
}

/// `ask_user` — delegates the agent's question to the [`AskHandler`].
#[must_use]
pub fn ask_user_tool(ask: Arc<dyn AskHandler>) -> SeherTool {
    let handler: ToolHandler = Arc::new(move |input: serde_json::Value| {
        let question = require_str(&input, "question")?;
        ask.ask_user(question).map_err(|e| e.to_string())
    });
    SeherTool::new(
        ASK_USER_TOOL,
        "Ask the user a clarifying question and get their answer. Use this whenever a \
         requirement is ambiguous instead of guessing.",
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user."
                }
            },
            "required": ["question"]
        }),
        handler,
    )
}

/// `submit_plan` — writes the full plan markdown to `plan_path`.
#[must_use]
pub fn submit_plan_tool(plan_path: PathBuf, plan_persisted: PlanPersistFlag) -> SeherTool {
    let handler: ToolHandler = Arc::new(move |input: serde_json::Value| {
        let content = require_str(&input, "content")?;
        write_plan(&plan_path, content)?;
        plan_persisted.store(true, Ordering::SeqCst);
        Ok("Plan saved.".to_string())
    });
    SeherTool::new(
        SUBMIT_PLAN_TOOL,
        "Submit the complete implementation plan as markdown. Call this once the plan is \
         ready; it overwrites the plan document.",
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The full plan, as markdown."
                }
            },
            "required": ["content"]
        }),
        handler,
    )
}

/// `update_plan` — find/replace a section of the existing plan document.
#[must_use]
pub fn update_plan_tool(plan_path: PathBuf, plan_persisted: PlanPersistFlag) -> SeherTool {
    let handler: ToolHandler = Arc::new(move |input: serde_json::Value| {
        let old = require_str(&input, "old")?;
        let new = require_str(&input, "new")?;
        let current = std::fs::read_to_string(&plan_path)
            .map_err(|e| format!("failed to read plan at {}: {e}", plan_path.display()))?;
        let updated = apply_update(&current, old, new)?;
        write_plan(&plan_path, &updated)?;
        plan_persisted.store(true, Ordering::SeqCst);
        Ok("Plan updated.".to_string())
    });
    SeherTool::new(
        UPDATE_PLAN_TOOL,
        "Revise the existing plan by replacing an exact snippet. `old` must match a unique \
         span of the current plan verbatim; if it does not match, re-read the plan and retry.",
        json!({
            "type": "object",
            "properties": {
                "old": {
                    "type": "string",
                    "description": "Exact text to replace (must occur exactly once)."
                },
                "new": {
                    "type": "string",
                    "description": "Replacement text."
                }
            },
            "required": ["old", "new"]
        }),
        handler,
    )
}

/// `generate_title` — captures the session title submitted by the agent.
#[must_use]
pub fn generate_title_tool(title_store: Arc<std::sync::Mutex<Option<String>>>) -> SeherTool {
    let handler: ToolHandler = Arc::new(move |input: serde_json::Value| {
        let title = require_str(&input, "title")?;
        let truncated: String = title.chars().take(80).collect();
        *title_store
            .lock()
            .map_err(|e| format!("title store lock poisoned: {e}"))? =
            Some(truncated.trim().to_string());
        Ok("Title saved.".to_string())
    });
    SeherTool::new(
        GENERATE_TITLE_TOOL,
        "Submit a concise session title (maximum 80 characters). Call this exactly once.",
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "A concise session title (max 80 characters)."
                }
            },
            "required": ["title"]
        }),
        handler,
    )
}

/// PR metadata captured by [`submit_pr_metadata_tool`].
#[derive(Debug, Clone)]
pub struct PrMetadata {
    pub title: String,
    pub body: String,
}

/// `submit_pr_metadata` — captures PR title and body submitted by the agent.
#[must_use]
pub fn submit_pr_metadata_tool(store: Arc<std::sync::Mutex<Option<PrMetadata>>>) -> SeherTool {
    let handler: ToolHandler = Arc::new(move |input: serde_json::Value| {
        let title = require_str(&input, "title")?;
        let body = require_str(&input, "body")?;
        *store
            .lock()
            .map_err(|e| format!("PR metadata store lock poisoned: {e}"))? = Some(PrMetadata {
            title: title.to_string(),
            body: body.to_string(),
        });
        Ok("PR metadata saved.".to_string())
    });
    SeherTool::new(
        SUBMIT_PR_METADATA_TOOL,
        "Submit the PR title and description. Call this exactly once after reviewing the changes.",
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "A concise PR title."
                },
                "body": {
                    "type": "string",
                    "description": "The PR description in markdown."
                }
            },
            "required": ["title", "body"]
        }),
        handler,
    )
}

/// Extract a required string field from the tool input JSON.
fn require_str<'a>(input: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    input
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("missing or non-string `{field}` argument"))
}

/// Write `content` to the plan file, rejecting blank content.
///
/// Blank content is rejected: an empty plan file is treated as "no plan" by
/// the plan-content fallback chain, which would silently re-expose the agent's
/// captured output as the plan and bypass the persist guard. Both plan-writing
/// tools go through here so the invariant cannot drift between them.
fn write_plan(plan_path: &std::path::Path, content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("plan content must not be empty".to_string());
    }
    std::fs::write(plan_path, content)
        .map_err(|e| format!("failed to write plan at {}: {e}", plan_path.display()))
}

/// Apply an exact-match find/replace to `current`.
///
/// `old` must occur **exactly once**:
/// - zero occurrences -> the snippet is stale; the caller should re-read.
/// - multiple occurrences -> ambiguous; the caller should widen the snippet.
fn apply_update(current: &str, old: &str, new: &str) -> Result<String, String> {
    if old.is_empty() {
        return Err("`old` must not be empty".to_string());
    }
    let count = current.matches(old).count();
    match count {
        0 => Err(
            "`old` text was not found in the current plan; re-read the plan and retry".to_string(),
        ),
        1 => Ok(current.replacen(old, new, 1)),
        n => Err(format!(
            "`old` text matched {n} times; provide a longer, unique snippet"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask_handler::ScriptedAskHandler;
    use tempfile::TempDir;

    fn invoke(tool: &SeherTool, input: serde_json::Value) -> Result<String, String> {
        (tool.handler)(input)
    }

    // -- apply_update (pure) --------------------------------------------------

    #[test]
    fn apply_update_replaces_unique_snippet() {
        let out = apply_update("# Plan\nUse JWT auth.\n", "JWT", "session")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(out, "# Plan\nUse session auth.\n");
    }

    #[test]
    fn apply_update_errors_when_not_found() {
        match apply_update("# Plan\n", "missing", "x") {
            Err(err) => assert!(err.contains("not found"), "got: {err}"),
            Ok(_) => panic!("expected error for stale snippet"),
        }
    }

    #[test]
    fn apply_update_errors_when_ambiguous() {
        match apply_update("a a", "a", "b") {
            Err(err) => assert!(err.contains("matched"), "got: {err}"),
            Ok(_) => panic!("expected error for ambiguous snippet"),
        }
    }

    #[test]
    fn apply_update_errors_on_empty_old() {
        assert!(apply_update("x", "", "y").is_err());
    }

    // -- submit_plan tool -----------------------------------------------------

    #[test]
    fn submit_plan_writes_content_to_file() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan = tmp.path().join("plan.md");
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = submit_plan_tool(plan.clone(), Arc::clone(&flag));
        let res = invoke(&tool, json!({"content": "# My Plan\nstep 1"}));
        assert!(res.is_ok(), "got: {res:?}");
        let written = std::fs::read_to_string(&plan).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(written, "# My Plan\nstep 1");
        assert!(
            flag.load(Ordering::SeqCst),
            "successful submit_plan must mark the plan as persisted"
        );
    }

    #[test]
    fn submit_plan_errors_without_content() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = submit_plan_tool(tmp.path().join("plan.md"), Arc::clone(&flag));
        assert!(invoke(&tool, json!({})).is_err());
        assert!(
            !flag.load(Ordering::SeqCst),
            "failed submit_plan must not mark the plan as persisted"
        );
    }

    #[test]
    fn submit_plan_error_does_not_set_persist_flag() {
        // Given: a plan path whose parent directory does not exist (write fails)
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = submit_plan_tool(PathBuf::from("/nonexistent/dir/plan.md"), Arc::clone(&flag));
        // When / Then: handler errors and the flag stays unset
        assert!(invoke(&tool, json!({"content": "x"})).is_err());
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn submit_plan_rejects_blank_content() {
        // Given: a valid plan path but whitespace-only content
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan = tmp.path().join("plan.md");
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = submit_plan_tool(plan.clone(), Arc::clone(&flag));
        // When / Then: handler errors, nothing is written, flag stays unset —
        // a blank plan file would fall through to the stdout fallback and
        // bypass the persist guard.
        let res = invoke(&tool, json!({"content": "  \n\t "}));
        assert!(res.is_err(), "blank content must error, got: {res:?}");
        assert!(!plan.exists());
        assert!(!flag.load(Ordering::SeqCst));
    }

    // -- update_plan tool -----------------------------------------------------

    #[test]
    fn update_plan_edits_existing_file() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan = tmp.path().join("plan.md");
        std::fs::write(&plan, "# Plan\nUse JWT.\n").unwrap_or_else(|e| panic!("{e:?}"));
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = update_plan_tool(plan.clone(), Arc::clone(&flag));
        let res = invoke(&tool, json!({"old": "JWT", "new": "sessions"}));
        assert!(res.is_ok(), "got: {res:?}");
        let written = std::fs::read_to_string(&plan).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(written, "# Plan\nUse sessions.\n");
        assert!(
            flag.load(Ordering::SeqCst),
            "successful update_plan must mark the plan as persisted"
        );
    }

    #[test]
    fn update_plan_errors_on_stale_old() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan = tmp.path().join("plan.md");
        std::fs::write(&plan, "# Plan\n").unwrap_or_else(|e| panic!("{e:?}"));
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = update_plan_tool(plan, Arc::clone(&flag));
        let res = invoke(&tool, json!({"old": "nope", "new": "x"}));
        assert!(
            res.is_err(),
            "stale old should error so the agent can retry"
        );
        assert!(
            !flag.load(Ordering::SeqCst),
            "failed update_plan must not mark the plan as persisted"
        );
    }

    #[test]
    fn update_plan_rejects_blank_result() {
        // Given: a plan the agent replaces wholesale with whitespace
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan = tmp.path().join("plan.md");
        std::fs::write(&plan, "# Plan\nfull content\n").unwrap_or_else(|e| panic!("{e:?}"));
        let flag: PlanPersistFlag = Arc::new(AtomicBool::new(false));
        let tool = update_plan_tool(plan.clone(), Arc::clone(&flag));
        // When / Then: handler errors, the original plan survives, and the
        // flag stays unset — a blank file would fall through to the stdout
        // fallback and bypass the persist guard.
        let res = invoke(
            &tool,
            json!({"old": "# Plan\nfull content\n", "new": " \n\t"}),
        );
        assert!(res.is_err(), "blank result must error, got: {res:?}");
        assert_eq!(
            std::fs::read_to_string(&plan).unwrap_or_else(|e| panic!("{e:?}")),
            "# Plan\nfull content\n",
            "original plan must be left intact"
        );
        assert!(!flag.load(Ordering::SeqCst));
    }

    // -- ask_user tool --------------------------------------------------------

    #[test]
    fn ask_user_delegates_to_handler() {
        let ask = Arc::new(ScriptedAskHandler::new(["the answer".to_string()]));
        let tool = ask_user_tool(ask);
        let res = invoke(&tool, json!({"question": "what?"}));
        assert_eq!(res.unwrap_or_else(|e| panic!("{e}")), "the answer");
    }

    #[test]
    fn ask_user_errors_without_question() {
        let ask = Arc::new(ScriptedAskHandler::new(["x".to_string()]));
        let tool = ask_user_tool(ask);
        assert!(invoke(&tool, json!({})).is_err());
    }

    // -- planning_tools set ---------------------------------------------------

    #[test]
    fn planning_tools_interactive_has_three() {
        let ask = Arc::new(ScriptedAskHandler::new(std::iter::empty()));
        let set = planning_tools(PathBuf::from("/tmp/plan.md"), ask, true);
        let names: Vec<&str> = set.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![ASK_USER_TOOL, SUBMIT_PLAN_TOOL, UPDATE_PLAN_TOOL]
        );
        assert!(!set.plan_persisted.load(Ordering::SeqCst));
    }

    #[test]
    fn planning_tools_noninteractive_has_submit_and_update() {
        // `ask_user` is the only user-facing tool; `update_plan` needs no
        // interaction and the fix-plan template relies on it for targeted
        // edits, so non-interactive runs register both plan-writing tools.
        let ask = Arc::new(ScriptedAskHandler::new(std::iter::empty()));
        let set = planning_tools(PathBuf::from("/tmp/plan.md"), ask, false);
        let names: Vec<&str> = set.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec![SUBMIT_PLAN_TOOL, UPDATE_PLAN_TOOL]);
    }

    // -- generate_title tool --------------------------------------------------

    #[test]
    fn generate_title_stores_title() {
        let store = Arc::new(std::sync::Mutex::new(None::<String>));
        let tool = generate_title_tool(Arc::clone(&store));
        let res = invoke(&tool, json!({"title": "Add session titles"}));
        assert!(res.is_ok(), "got: {res:?}");
        assert_eq!(
            store.lock().unwrap_or_else(|e| panic!("{e:?}")).as_deref(),
            Some("Add session titles")
        );
    }

    #[test]
    fn generate_title_truncates_long_title() {
        let store = Arc::new(std::sync::Mutex::new(None::<String>));
        let tool = generate_title_tool(Arc::clone(&store));
        let long = "a".repeat(100);
        let res = invoke(&tool, json!({"title": long}));
        assert!(res.is_ok(), "got: {res:?}");
        assert_eq!(
            store
                .lock()
                .unwrap_or_else(|e| panic!("{e:?}"))
                .as_ref()
                .unwrap_or_else(|| panic!("expected Some"))
                .len(),
            80
        );
    }

    #[test]
    fn generate_title_errors_without_title() {
        let store = Arc::new(std::sync::Mutex::new(None::<String>));
        let tool = generate_title_tool(store);
        assert!(invoke(&tool, json!({})).is_err());
    }

    // -- submit_pr_metadata tool ----------------------------------------------

    #[test]
    fn submit_pr_metadata_stores_title_and_body() {
        let store = Arc::new(std::sync::Mutex::new(None::<PrMetadata>));
        let tool = submit_pr_metadata_tool(Arc::clone(&store));
        let res = invoke(&tool, json!({"title": "fix: bug", "body": "Fixes #42"}));
        assert!(res.is_ok(), "got: {res:?}");
        let meta = store
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"))
            .clone()
            .unwrap_or_else(|| panic!("expected Some"));
        assert_eq!(meta.title, "fix: bug");
        assert_eq!(meta.body, "Fixes #42");
    }

    #[test]
    fn submit_pr_metadata_errors_without_title() {
        let store = Arc::new(std::sync::Mutex::new(None::<PrMetadata>));
        let tool = submit_pr_metadata_tool(store);
        assert!(invoke(&tool, json!({"body": "x"})).is_err());
    }

    #[test]
    fn submit_pr_metadata_errors_without_body() {
        let store = Arc::new(std::sync::Mutex::new(None::<PrMetadata>));
        let tool = submit_pr_metadata_tool(store);
        assert!(invoke(&tool, json!({"title": "x"})).is_err());
    }
}
