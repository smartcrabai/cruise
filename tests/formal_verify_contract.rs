use cruise::formal_verify::{
    BlockReport, Language, StageReport, Verdict, VerifyReport, aggregate,
    alloy::{alloy_commands, receipt_verdict},
    extract_blocks, verify_blocks,
    quint::{main_module, quint_verdict},
};
use std::path::PathBuf;

#[test]
fn extracts_only_closed_exact_language_fences() {
    let input = "```quint\nfirst\n```\n```alloy\nsecond\n```\n```Quint\nignored\n```\n```quint extra\nignored\n```\n~~~quint\ntildes\n~~~\n````quint\ninner ```\n````\n```alloy\nunclosed\n";
    let found = extract_blocks(input);
    assert_eq!(found.blocks.len(), 4);
    assert_eq!(
        found
            .blocks
            .iter()
            .map(|b| (b.language, b.number, b.source.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (Language::Quint, 1, "first\n"),
            (Language::Alloy, 1, "second\n"),
            (Language::Quint, 2, "tildes\n"),
            (Language::Quint, 3, "inner ```\n")
        ]
    );
    assert!(found.over_limit.is_empty());
}

#[test]
fn extracts_container_fences_and_rejects_implicit_ends() {
    let input = "> ~~~quint\n> quoted\n> ~~~~\n\n- ```alloy\n  listed\n  ```\n\n> ```quint\n> unclosed\n\noutside\n\n~~~alloy\nnot closed\n~~\n";
    let found = extract_blocks(input);
    assert_eq!(
        found.blocks.iter().map(|b| (b.language, b.number, b.source.as_str())).collect::<Vec<_>>(),
        vec![(Language::Quint, 1, "quoted\n"), (Language::Alloy, 1, "listed\n")]
    );
    assert!(found.over_limit.is_empty());
}

#[test]
fn empty_closed_fences_and_unclosed_openers_are_distinguished() {
    for input in ["```quint\n```", "~~~quint\n~~~", "> ```quint\n> ```", "~~~quint\r~~~", "```quint\r\n```"] {
        let found = extract_blocks(input);
        assert_eq!(found.blocks.len(), 1, "{input:?}");
        assert_eq!(found.blocks[0].source, "");
    }
    for input in ["```quint", "~~~quint\n", "> ```quint\n> text\n", "````quint\n```\n"] {
        assert!(extract_blocks(input).blocks.is_empty(), "{input:?}");
    }
}

#[test]
fn extraction_limits_are_per_language_and_keep_encounter_order() {
    let input = (0..6)
        .map(|i| format!("```quint\nq{i}\n```\n```alloy\na{i}\n```\n"))
        .collect::<String>();
    let found = extract_blocks(&input);
    assert_eq!(found.blocks.len(), 8);
    for (i, pair) in found.blocks.chunks_exact(2).enumerate() {
        assert_eq!((pair[0].language, pair[0].number), (Language::Quint, i + 1));
        assert_eq!((pair[1].language, pair[1].number), (Language::Alloy, i + 1));
    }
    assert_eq!(
        found.over_limit,
        vec![
            Language::Quint,
            Language::Alloy,
            Language::Quint,
            Language::Alloy
        ]
    );
}

#[test]
fn verify_flow_reports_nothing_for_unclosed_fences() {
    let dir = tempfile::tempdir().unwrap();
    let blocks = extract_blocks("> ~~~quint\n> unclosed\n\noutside\n\n```alloy\nunclosed\n");
    let report = verify_blocks(blocks, dir.path(), std::time::Duration::from_secs(1), None).unwrap();
    assert!(report.blocks.is_empty());
    assert_eq!(report.verdict(), Verdict::Error);
    assert!(std::fs::read_to_string(report.artifact.join("report.md"))
        .unwrap()
        .contains("nothing was verified"));
}

#[test]
fn aggregate_and_reports_prioritize_failure_then_error() {
    for (input, expected) in [
        (&[][..], Verdict::Error),
        (&[Verdict::Passed][..], Verdict::Passed),
        (&[Verdict::Passed, Verdict::Error][..], Verdict::Error),
        (
            &[Verdict::Error, Verdict::Failed, Verdict::Passed][..],
            Verdict::Failed,
        ),
    ] {
        assert_eq!(aggregate(input), expected);
    }
    let stage = |verdict| StageReport {
        name: "check".into(),
        verdict,
        message: String::new(),
        timed_out: false,
        log: String::new(),
    };
    let block = |verdict| BlockReport {
        language: Language::Quint,
        number: 1,
        stages: vec![stage(verdict)],
        artifact: PathBuf::from("artifact"),
    };
    let report = VerifyReport {
        blocks: vec![block(Verdict::Error), block(Verdict::Failed)],
        artifact: PathBuf::from("artifact"),
        warnings: vec![],
    };
    assert_eq!(report.verdict(), Verdict::Failed);
    assert!(
        report
            .render_markdown()
            .contains("Formal verification: failed")
    );
}

#[test]
fn quint_exit_timeout_and_violation_precedence() {
    for (exit, output, timed_out, expected) in [
        (Some(0), "", false, Verdict::Passed),
        (Some(0), "[violation]", false, Verdict::Passed),
        (Some(1), "[violation]", false, Verdict::Failed),
        (Some(1), "other error", false, Verdict::Error),
        (None, "", false, Verdict::Error),
        (Some(1), "[violation]", true, Verdict::Error),
    ] {
        assert_eq!(quint_verdict(exit, output, timed_out), expected);
    }
}

#[test]
fn quint_ast_selects_main_and_qualified_targets() {
    let ast = r#"{"modules":[{"name":"helper","declarations":[]},{"name":"Main","declarations":[{"kind":"def","qualifier":"action","name":"init"},{"kind":"def","qualifier":"action","name":"step"},{"kind":"def","qualifier":"val","name":"invSafe"},{"kind":"def","qualifier":"temporal","name":"propLive"},{"kind":"def","qualifier":"action","name":"invWrong"},{"kind":"def","qualifier":"val","name":"propWrong"},{"kind":"other","qualifier":"val","name":"invOther"}]}]}"#;
    let m = main_module(ast).unwrap().unwrap();
    assert_eq!(m.name, "Main");
    assert_eq!(m.invariants, ["invSafe"]);
    assert_eq!(m.temporals, ["propLive"]);
    assert!(main_module(r#"{"modules":[]}"#).unwrap().is_none());
    assert!(main_module("not json").is_err());
    assert!(main_module(r#"{"modules":{}}"#).is_err());
    let duplicate = r#"{"modules":[{"name":"A","declarations":[{"kind":"def","qualifier":"action","name":"init"},{"kind":"def","qualifier":"action","name":"step"}]},{"name":"B","declarations":[{"kind":"def","qualifier":"action","name":"init"},{"kind":"def","qualifier":"action","name":"step"}]}]}"#;
    assert!(main_module(duplicate).unwrap_err().contains("ambiguous"));
}

#[test]
fn alloy_command_listing_requires_ordered_valid_entries() {
    let listing = "0 . Run Witness for 3\n1 . Check Safety for 4\n2 . Run run$3 for 3\n3 . Run labeled for 3\n";
    let commands = alloy_commands(listing).unwrap();
    assert_eq!(
        commands.iter().map(|c| (c.kind.as_str(), c.name.as_str())).collect::<Vec<_>>(),
        vec![("run", "Witness"), ("check", "Safety"), ("run", "run$3"), ("run", "labeled")]
    );
    assert!(alloy_commands("1 . Run Missing\n").is_err());
    assert!(alloy_commands("").is_err());
    assert!(alloy_commands("0 . Unknown thing\n").is_err());
}

#[test]
fn alloy_receipts_require_matching_command_and_valid_witnesses() {
    let sat = r#"{"commands":{"C":{"type":"run","solution":[{"instances":[{}]}]}}}"#;
    let unsat = r#"{"commands":{"C":{"type":"run"}}}"#;
    assert_eq!(receipt_verdict("run", "C", sat), Verdict::Passed);
    assert_eq!(receipt_verdict("run", "C", unsat), Verdict::Failed);
    let sat_check = sat.replace("run", "check");
    let unsat_check = unsat.replace("run", "check");
    assert_eq!(receipt_verdict("check", "C", &sat_check), Verdict::Failed);
    assert_eq!(receipt_verdict("check", "C", &unsat_check), Verdict::Passed);
    assert_eq!(receipt_verdict("check", "C", sat), Verdict::Error);
    assert_eq!(receipt_verdict("run", "missing", sat), Verdict::Error);
    assert_eq!(receipt_verdict("run", "C", "bad json"), Verdict::Error);
    for solution in [
        "null",
        "{}",
        "[]",
        "[{}]",
        "[{\"instances\":[]}]",
        "[{\"instances\":null}]",
        "[{\"instances\":[{}]},{}]",
    ] {
        let receipt = format!(r#"{{"commands":{{"C":{{"type":"run","solution":{solution}}}}}}}"#);
        assert_eq!(
            receipt_verdict("run", "C", &receipt),
            Verdict::Error,
            "{solution}"
        );
    }
}
