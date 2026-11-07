//! Entry-point contract for the opt-in prompt evaluation driver.
use std::process::Command;

#[test]
fn ignored_live_evaluation_rejects_missing_opt_in() {
    let output = Command::new(env!("CARGO"))
        .args(["test", "--all-features", "--test", "prompt_eval", "--", "--ignored", "--nocapture"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("CRUISE_PROMPT_EVAL")
        .env_remove("CRUISE_PROMPT_EVAL_MODEL")
        .env_remove("CRUISE_PROMPT_EVAL_JUDGE_MODEL")
        .output()
        .expect("cargo must be available to run the prompt-eval entry point");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "missing opt-in must not report success: {stdout}\n{stderr}");
    assert!(
        format!("{stdout}\n{stderr}").contains("CRUISE_PROMPT_EVAL"),
        "the failure must identify the missing opt-in, not a build or setup failure: {stdout}\n{stderr}"
    );
}
