// Public documentation must enumerate each supported configuration override.
#[test]
fn documented_configuration_overrides_cover_the_public_contract() {
    let readme = include_str!("../README.md");
    let skill = include_str!("../skills/cruise-cli/SKILL.md");
    let names = [
        "CRUISE_MODEL",
        "CRUISE_PLAN_MODEL",
        "CRUISE_SUBAGENT_MODEL",
        "CRUISE_SDK",
        "CRUISE_COMMAND",
        "CRUISE_MAX_RETRIES",
        "CRUISE_INTERACTIVE_PLANNING",
        "CRUISE_CLEANUP_AFTER_PR",
        "CRUISE_FORCE_EXEC",
        "CRUISE_COMPUTER_USE",
        "CRUISE_PERMISSION",
        "CRUISE_DISALLOWED_TOOLS",
        "CRUISE_RETRY",
        "CRUISE_RETRY_BASE_DELAY_MS",
        "CRUISE_RETRY_MAX_DELAY_MS",
        "CRUISE_RETRY_MODEL_FALLBACK",
        "CRUISE_RETRY_FALLBACK_CHAINS",
        "CRUISE_LANGUAGES",
        "CRUISE_LANGUAGE_PR",
        "CRUISE_LANGUAGE_PLAN",
        "CRUISE_RUN_ALL_PARALLELISM",
    ];
    for (document, text) in [("README.md", readme), ("skills/cruise-cli/SKILL.md", skill)] {
        for name in names {
            assert!(
                text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|word| word == name),
                "{document} does not document {name}"
            );
        }
    }
}
