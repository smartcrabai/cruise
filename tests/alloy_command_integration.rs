//! Production-connected Alloy CLI integration, using an explicitly supplied pinned JAR.
use cruise::formal_verify::{Language, SpecBlock, Verdict, alloy};
use std::time::Duration;

#[test]
#[ignore = "requires Alloy 6.2.0 JAR and Java"]
fn alloy_discovers_and_executes_every_command() -> Result<(), Box<dyn std::error::Error>> {
    // This integration requires the pinned distribution and a compatible Java on PATH.
    assert!(
        std::env::var_os("CRUISE_ALLOY_JAR").is_some(),
        "set CRUISE_ALLOY_JAR to Alloy 6.2.0"
    );
    let dir = tempfile::tempdir()?;
    let block = SpecBlock {
        language: Language::Alloy,
        number: 1,
        source: "pred witness { some univ } assert safe { no univ } run witness for 3 check safe for 3\nrun { some univ } for 3\nlabeled: run { some univ } for 3\n".into(),
    };
    let report = alloy::verify(&block, dir.path(), Duration::from_secs(60), None);
    assert_eq!(report.stages.len(), 4, "{:?}", report.stages);
    assert_eq!(
        report.stages.iter().map(|s| s.verdict).collect::<Vec<_>>(),
        vec![
            Verdict::Passed,
            Verdict::Failed,
            Verdict::Passed,
            Verdict::Passed
        ],
        "{:?}",
        report.stages
    );
    for index in 0..4 {
        assert!(
            dir.path()
                .join(format!("command-{index}/receipt.json"))
                .exists()
        );
    }
    Ok(())
}
