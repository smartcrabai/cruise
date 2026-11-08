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
