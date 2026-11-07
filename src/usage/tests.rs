use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn record<'a>() -> UsageRecord<'a> {
    UsageRecord {
        v: 1,
        ts: "2026-11-05T10:00:00Z".into(),
        session_id: Some("session"),
        step: Some("build"),
        purpose: "workflow_step",
        backend: "command",
        model: None,
        attempt: 1,
        outcome: "success",
        usage: SerializedUsage::Unavailable {
            reason: "command_backend",
        },
    }
}

#[test]
fn counts_merge_marks_metric_unknown_when_any_round_lacks_it() {
    let mut accumulator = UsageAccumulator::default();
    for counts in [
        UsageCounts {
            input_tokens: Some(10),
            ..Default::default()
        },
        UsageCounts {
            input_tokens: Some(5),
            cache_read_input_tokens: Some(3),
            ..Default::default()
        },
    ] {
        accumulator.add(UsageRound {
            model: None,
            counts,
        });
    }
    let UsageField::Reported { totals, by_model } = accumulator.into_field("not_reported") else {
        panic!("expected reported usage")
    };
    assert_eq!(totals.input_tokens, Some(15));
    assert_eq!(totals.cache_read_input_tokens, None);
    assert_eq!(by_model, None);
}

#[test]
fn accumulator_into_field_reports_unavailable_without_rounds() {
    let mut accumulator = UsageAccumulator::default();
    accumulator.add(UsageRound {
        model: None,
        counts: UsageCounts::default(),
    });
    assert_eq!(
        accumulator.into_field("not_reported"),
        UsageField::Unavailable {
            reason: "not_reported"
        }
    );
    let mut accumulator = UsageAccumulator::default();
    accumulator.add(UsageRound {
        model: Some("model".into()),
        counts: UsageCounts {
            input_tokens: Some(0),
            ..Default::default()
        },
    });
    assert!(matches!(
        accumulator.into_field("not_reported"),
        UsageField::Reported {
            by_model: Some(_),
            ..
        }
    ));
}

#[test]
fn record_unavailable_serializes_without_numeric_keys() {
    let value = serde_json::to_value(record()).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(
        value["usage"],
        json!({"status": "unavailable", "reason": "command_backend"})
    );
}

#[test]
fn record_key_set_is_whitelisted() {
    let counts = UsageCounts {
        input_tokens: Some(1),
        ..Default::default()
    };
    let mut rec = record();
    rec.usage = SerializedUsage::Reported {
        totals: &counts,
        by_model: None,
    };
    let value = serde_json::to_value(rec).unwrap_or_else(|e| panic!("{e:?}"));
    let keys: BTreeSet<_> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected object"))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "v",
            "ts",
            "session_id",
            "step",
            "purpose",
            "backend",
            "model",
            "attempt",
            "outcome",
            "usage"
        ]
        .into_iter()
        .collect()
    );
    let allowed: BTreeSet<_> = [
        "status",
        "input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "cost_usd",
        "by_model",
        "reason",
    ]
    .into_iter()
    .collect();
    assert!(
        value["usage"]
            .as_object()
            .unwrap_or_else(|| panic!("expected object"))
            .keys()
            .all(|k| allowed.contains(k.as_str()))
    );
}

#[cfg(unix)]
#[test]
fn append_record_creates_private_dir_and_file() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
    let dir = temp.path().join("private");
    let path = dir.join("usage.ndjson");
    append_record(&path, &record()).unwrap_or_else(|e| panic!("{e:?}"));
    let first = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e:?}"));
    append_record(&path, &record()).unwrap_or_else(|e| panic!("{e:?}"));
    let both = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(both, format!("{first}{first}"));
    assert_eq!(both.lines().count(), 2);
    assert_eq!(
        std::fs::metadata(dir)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn append_record_concurrent_threads_yield_only_whole_lines() {
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
    let path = temp.path().join("private/usage.ndjson");
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let path = &path;
            scope.spawn(move || {
                for _ in 0..200 {
                    append_record(path, &record()).unwrap_or_else(|e| panic!("{e:?}"));
                }
            });
        }
    });
    let data = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(data.lines().count(), 1600);
    for line in data.lines() {
        serde_json::from_str::<Value>(line).unwrap_or_else(|e| panic!("{e:?}"));
    }
}

#[test]
fn recording_enabled_parses_env() {
    use crate::test_support::{EnvGuard, lock_process};
    let _lock = lock_process();
    for value in ["0", "false", " FALSE "] {
        let _guard = EnvGuard::set("CRUISE_USAGE_RECORDING", value);
        assert!(!recording_enabled(), "{value}");
    }
    for value in ["1", "true", "maybe"] {
        let _guard = EnvGuard::set("CRUISE_USAGE_RECORDING", value);
        assert!(recording_enabled(), "{value}");
    }
    let _guard = EnvGuard::remove("CRUISE_USAGE_RECORDING");
    assert!(recording_enabled());
}

#[test]
fn record_attempt_never_panics_on_unwritable_path() {
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
    let parent = temp.path().join("not_a_directory");
    std::fs::write(&parent, "file").unwrap_or_else(|e| panic!("{e:?}"));
    let guard = OnceLock::new();
    assert!(guard.get().is_none());
    for _ in 0..2 {
        record_attempt_with_warn_guard(&record(), Ok(parent.join("usage.ndjson")), &guard);
        assert!(guard.get().is_some());
    }
    assert!(!parent.join("usage.ndjson").exists());
}

#[test]
fn warning_stderr_isolated_process() {
    const CHILD: &str = "CRUISE_USAGE_WARNING_TEST_CHILD";
    if let Ok(case) = std::env::var(CHILD) {
        if case == "switch" {
            assert!(recording_enabled());
        } else {
            let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
            let parent = temp.path().join("file");
            std::fs::write(&parent, "file").unwrap_or_else(|e| panic!("{e:?}"));
            let guard = OnceLock::new();
            for _ in 0..2 {
                record_attempt_with_warn_guard(&record(), Ok(parent.join("usage.ndjson")), &guard);
            }
        }
        return;
    }
    for (case, warning) in [
        ("switch", "warning: invalid CRUISE_USAGE_RECORDING value, recording enabled"),
        ("write", "warning: usage record skipped:"),
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "usage::tests::warning_stderr_isolated_process", "--nocapture"])
            .env(CHILD, case)
            .env("CRUISE_USAGE_RECORDING", "invalid")
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr.matches(warning).count(), 1, "{stderr}");
    }
}
