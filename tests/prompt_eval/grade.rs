//! Strict judge response parsing regression coverage.
use super::{Rubric, parse_judgment};

#[test]
fn judge_rejects_unknown_fields_extra_text_and_missing_ids() {
    let rubric = vec![
        Rubric {
            id: "first".into(),
            text: "First".into(),
        },
        Rubric {
            id: "second".into(),
            text: "Second".into(),
        },
    ];
    let valid = r#"{"verdicts":[{"id":"first","pass":true,"evidence":"ok"},{"id":"second","pass":false,"evidence":"missing"}]}"#;
    assert!(parse_judgment(valid, &rubric).is_ok());
    assert!(parse_judgment(&format!("{valid} trailing"), &rubric).is_err());
    assert!(
        parse_judgment(
            r#"{"verdicts":[{"id":"first","pass":true,"evidence":"ok"}]}"#,
            &rubric
        )
        .is_err()
    );
    assert!(parse_judgment(r#"{"verdicts":[{"id":"first","pass":true,"evidence":"ok","extra":1},{"id":"second","pass":false,"evidence":"no"}]}"#, &rubric).is_err());
    assert!(parse_judgment(&format!("```json\n{valid}\n```\nextra"), &rubric).is_err());
}
