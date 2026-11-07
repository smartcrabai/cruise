//! Catalog validation regression tests against the actual loaded cases.
use super::catalog::{Check, load, validate};

#[test]
fn rejects_invalid_case_settings_and_duplicate_rubrics() -> Result<(), String> {
    let cases = load()?;
    validate(&cases)?;
    let mut invalid = cases.clone();
    invalid[0].repeat = Some(0);
    assert!(validate(&invalid).is_err());
    let mut invalid = cases.clone();
    invalid[0].checks.push(Check::Matches {
        matches: "[".into(),
    });
    assert!(validate(&invalid).is_err());
    let mut invalid = cases.clone();
    let duplicated = invalid[0].rubric[0].clone();
    invalid[1].rubric.push(duplicated);
    assert!(validate(&invalid).is_err());
    let mut invalid = cases;
    let language = invalid
        .iter_mut()
        .find_map(|c| c.language.as_mut())
        .ok_or("missing language case")?;
    language.values.push("French".into());
    assert!(validate(&invalid).is_err());
    Ok(())
}

#[test]
fn case_unknown_fields_are_rejected() {
    let yaml = "id: example\ntarget: {prompt_file: prompts/plan.md}\nrender: plan\noutput: text\nrubric: []\nunexpected: true\n";
    assert!(serde_yaml::from_str::<super::catalog::Case>(yaml).is_err());
}

#[test]
fn repository_cases_have_fixtures_and_edit_cases_capture_diffs() -> Result<(), String> {
    let cases = load()?;
    for case in &cases {
        if case.target.workflow_step.is_some() {
            assert!(
                case.workspace
                    .as_ref()
                    .is_some_and(|w| super::root().join(w).join("src/lib.rs").is_file()),
                "{}",
                case.id
            );
        }
    }
    for id in [
        "tdd-write-tests",
        "tdd-implement",
        "implement-after-tests-file",
    ] {
        let case = cases
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("missing case {id}"))?;
        assert_eq!(case.output, "workspace_diff", "{id}");
        let workspace = super::root().join(
            case.workspace
                .as_ref()
                .ok_or_else(|| format!("missing workspace for {id}"))?,
        );
        assert!(workspace.join("Cargo.toml").is_file());
        if id != "tdd-write-tests" {
            assert!(workspace.join("tests/add.rs").is_file());
        }
    }
    Ok(())
}
