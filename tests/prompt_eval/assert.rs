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
    assert!(check_output(&check, "\u{65e5}\u{672c}"));
    assert!(!check_output(&check, "\u{65e5}"));
}

#[test]
fn language_thresholds_and_empty_text() {
    let english = "Plan the change in src/a.rs:10";
    let japanese = "\u{5909}\u{66f4}\u{3092}\u{8a08}\u{753b}\u{3057}\u{307e}\u{3059}\u{3002}\u{30c6}\u{30b9}\u{30c8}\u{3092}\u{8ffd}\u{52a0}\u{3057}\u{307e}\u{3059}\u{3002}";
    assert!(language_ok("English", english));
    assert!(!language_ok("Japanese", english));
    assert!(language_ok("Japanese", japanese));
    assert!(!language_ok("English", japanese));
    for empty in ["", " \n\t"] {
        assert!(!language_ok("English", empty));
        assert!(!language_ok("Japanese", empty));
    }
    assert!(language_ok(
        "Japanese",
        &format!("\u{65e5}{}", "a".repeat(9))
    ));
    assert!(!language_ok(
        "Japanese",
        &format!("\u{65e5}{}", "a".repeat(10))
    ));
    assert!(language_ok(
        "English",
        &format!("\u{65e5}{}", "a".repeat(99))
    ));
    assert!(!language_ok(
        "English",
        &format!("\u{65e5}{}", "a".repeat(98))
    ));
    assert!(!language_ok("Other", english));
}
