#[path = "prompt_eval/mod.rs"]
mod prompt_eval;

#[test]
fn catalog_contract() {
    prompt_eval::catalog_contract();
}

#[test]
#[ignore = "calls live models and requires explicit opt-in"]
fn live_prompt_evaluation() {
    assert_eq!(
        std::env::var("CRUISE_PROMPT_EVAL").as_deref(),
        Ok("1"),
        "set CRUISE_PROMPT_EVAL=1 to run live evaluation"
    );
    let runtime = tokio::runtime::Runtime::new().expect("create runtime");
    runtime
        .block_on(prompt_eval::run())
        .unwrap_or_else(|e| panic!("prompt evaluation failed: {e}"));
}
