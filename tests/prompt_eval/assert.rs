//! Offline regression coverage for deterministic output checks.
use super::catalog::{Check, check_output, language_ok};

#[test]
fn substring_and_regex_checks_are_case_sensitive() {
    assert!(check_output(
        &Check::Contains {
            contains: "Ab".into()
        },
        "xAbc"
    ));
    assert!(!check_output(
        &Check::Contains {
            contains: "ab".into()
        },
        "xAbc"
    ));
    assert!(check_output(
        &Check::NotContains {
            not_contains: "ab".into()
        },
        "xAbc"
    ));
    assert!(check_output(
        &Check::Matches {
            matches: "^A.+c$".into()
        },
        "Abc"
    ));
    assert!(!check_output(
        &Check::NotMatches {
            not_matches: "^A.+c$".into()
        },
        "Abc"
    ));
    assert!(!check_output(
        &Check::Matches {
            matches: "[".into()
        },
        "anything"
    ));
    assert!(!check_output(
        &Check::NotMatches {
            not_matches: "[".into()
        },
        "anything"
    ));
}

#[test]
fn minimum_counts_unicode_scalars() {
    let check = Check::MinChars { min_chars: 2 };
    assert!(check_output(&check, "日本"));
    assert!(!check_output(&check, "日"));
}

#[test]
fn language_thresholds_and_empty_text() {
    let english = "Plan the change in src/a.rs:10";
    let japanese = "変更を計画します。テストを追加します。";
    assert!(language_ok("English", english));
    assert!(!language_ok("Japanese", english));
    assert!(language_ok("Japanese", japanese));
    assert!(!language_ok("English", japanese));
    for empty in ["", " \n\t"] {
        assert!(!language_ok("English", empty));
        assert!(!language_ok("Japanese", empty));
    }
    assert!(language_ok("Japanese", &format!("日{}", "a".repeat(9))));
    assert!(!language_ok("Japanese", &format!("日{}", "a".repeat(10))));
    assert!(language_ok("English", &format!("日{}", "a".repeat(99))));
    assert!(!language_ok("English", &format!("日{}", "a".repeat(98))));
    assert!(!language_ok("Other", english));
}
