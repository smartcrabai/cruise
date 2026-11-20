use std::process::Command;

#[path = "prompt_eval/mod.rs"]
mod prompt_eval;

#[test]
fn catalog_contract() -> Result<(), Box<dyn std::error::Error>> {
    prompt_eval::catalog_contract()
}

#[test]
#[ignore = "calls live models and requires explicit opt-in"]
fn live_prompt_evaluation() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        std::env::var("CRUISE_PROMPT_EVAL").as_deref(),
        Ok("1"),
        "set CRUISE_PROMPT_EVAL=1 to run live evaluation"
    );
    let runtime = tokio::runtime::Runtime::new()?;
    runtime
        .block_on(prompt_eval::run())
        .map_err(|e| format!("prompt evaluation failed: {e}"))?;
    Ok(())
}

#[test]
fn ignored_live_evaluation_rejects_missing_opt_in() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--ignored",
            "--exact",
            "live_prompt_evaluation",
            "--nocapture",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("CRUISE_PROMPT_EVAL")
        .env_remove("CRUISE_PROMPT_EVAL_MODEL")
        .env_remove("CRUISE_PROMPT_EVAL_JUDGE_MODEL")
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "missing opt-in must not report success: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("set CRUISE_PROMPT_EVAL=1 to run live evaluation")
            || stderr.contains("set CRUISE_PROMPT_EVAL=1 to run live evaluation"),
        "the failure must identify the missing opt-in, not a build or setup failure: {stdout}\n{stderr}"
    );
    Ok(())
}
