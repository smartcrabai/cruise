#![cfg(unix)]
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "isolated executable contract fixtures"
)]

use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Fixture(TempDir);

impl Fixture {
    fn new() -> Self {
        Self(TempDir::new().unwrap())
    }

    fn write_lines(&self, lines: &[String]) {
        let dir = self.0.path().join("state/cruise");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("usage.ndjson"), format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn write_records(&self, records: &[Value]) {
        self.write_lines(&records.iter().map(Value::to_string).collect::<Vec<_>>());
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(self.0.path())
            .env("HOME", self.0.path())
            .env("XDG_STATE_HOME", self.0.path().join("state"))
            .env("XDG_CONFIG_HOME", self.0.path().join("config"))
            .env("XDG_DATA_HOME", self.0.path().join("data"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .arg("usage")
            .args(args)
            .output()
            .expect("run cruise usage")
    }

    fn success(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "status: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        serde_json::from_str(&self.success(&args)).expect("valid JSON output")
    }
}

fn record(session: &str, step: &str, backend: &str, model: Option<&str>, usage: &Value) -> Value {
    json!({"v": 1, "ts": "2026-11-05T10:00:00Z", "session_id": session,
        "step": step, "purpose": "workflow_step", "backend": backend, "model": model,
        "attempt": 1, "outcome": "success", "usage": usage})
}

fn unavailable() -> Value {
    json!({"status": "unavailable", "reason": "command_backend"})
}

fn fixture_records() -> Vec<Value> {
    vec![
        record(
            "one",
            "build",
            "claude",
            Some("sonnet"),
            &json!({"status":"reported", "input_tokens":10, "output_tokens":4, "cost_usd":0.5}),
        ),
        record(
            "one",
            "build",
            "claude",
            Some("sonnet"),
            &json!({"status":"reported", "input_tokens":20, "output_tokens":6, "cost_usd":0.25}),
        ),
        record("one", "build", "command", None, &unavailable()),
        record(
            "other",
            "test",
            "jcode",
            Some("model-b"),
            &json!({"status":"reported", "input_tokens":7, "output_tokens":3}),
        ),
    ]
}

#[test]
fn usage_without_file_prints_empty_result_and_exits_zero() {
    let f = Fixture::new();
    let text = f.success(&[]);
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert_eq!(
        text.lines().next(),
        Some(
            "step  backend  model  calls  unavailable  input  output  cache_read  cache_creation  reasoning  total  cost_usd"
        )
    );
    let value = f.json(&[]);
    assert_eq!(value["schema"], 1);
    assert_eq!(value["group_by"], json!(["step", "backend", "model"]));
    assert_eq!(value["rows"], json!([]));
    assert_eq!(value["malformed_lines"], 0);
    assert_eq!(value["unsupported_version_lines"], 0);
}

#[test]
fn usage_json_groups_by_step_backend_model() {
    let f = Fixture::new();
    f.write_records(&fixture_records());
    let value = f.json(&["--by", "step,backend,model"]);
    assert_eq!(
        value,
        json!({"schema":1, "group_by":["step","backend","model"], "rows":[
        {"step":"build", "backend":"claude", "model":"sonnet", "calls":2, "unavailable_calls":0, "reported_calls":2, "input_tokens":30, "output_tokens":10, "cache_read_input_tokens":null, "cache_creation_input_tokens":null, "reasoning_tokens":null, "total_tokens":40, "cost_usd":0.75},
        {"step":"build", "backend":"command", "model":null, "calls":1, "unavailable_calls":1, "reported_calls":0, "input_tokens":null, "output_tokens":null, "cache_read_input_tokens":null, "cache_creation_input_tokens":null, "reasoning_tokens":null, "total_tokens":null, "cost_usd":null},
        {"step":"test", "backend":"jcode", "model":"model-b", "calls":1, "unavailable_calls":0, "reported_calls":1, "input_tokens":7, "output_tokens":3, "cache_read_input_tokens":null, "cache_creation_input_tokens":null, "reasoning_tokens":null, "total_tokens":10, "cost_usd":null}
    ], "malformed_lines":0, "unsupported_version_lines":0})
    );
}

#[test]
fn usage_csv_header_and_empty_cells_for_unreported() {
    let f = Fixture::new();
    f.write_records(&[record("one", "build", "command", None, &unavailable())]);
    let text = f.success(&["--format", "csv"]);
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some(
            "step,backend,model,calls,unavailable_calls,reported_calls,input_tokens,output_tokens,cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,total_tokens,cost_usd"
        )
    );
    assert_eq!(lines.next(), Some("build,command,,1,1,0,,,,,,,"));
    assert_eq!(lines.next(), None);
}

#[test]
fn usage_text_prints_dash_for_unreported_and_footnote_for_partial() {
    let f = Fixture::new();
    f.write_records(&fixture_records());
    let text = f.success(&[]);
    assert!(
        text.lines()
            .any(|line| line.contains("command") && line.contains('-')),
        "{text}"
    );
    assert!(text.to_lowercase().contains("lower bound"), "{text}");
}

#[test]
fn usage_session_filter_limits_rows() {
    let f = Fixture::new();
    f.write_records(&fixture_records());
    let value = f.json(&["--session", "other"]);
    let rows = value["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["step"], "test");
    assert_eq!(rows[0]["input_tokens"], 7);
}

#[test]
fn usage_rejects_unknown_by_dimension() {
    let output = Fixture::new().run(&["--by", "bogus"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    for dim in ["session", "step", "purpose", "backend", "model"] {
        assert!(stderr.contains(dim), "missing {dim}: {stderr}");
    }
}

#[test]
fn summarize_skips_malformed_and_unsupported_version_lines() {
    let f = Fixture::new();
    let mut records = fixture_records();
    records.truncate(3);
    let mut lines: Vec<String> = records.iter().map(Value::to_string).collect();
    lines.push("{not json".into());
    let mut future = records[0].clone();
    future["v"] = json!(2);
    lines.push(future.to_string());
    f.write_lines(&lines);
    let value = f.json(&[]);
    assert_eq!(value["malformed_lines"], 1);
    assert_eq!(value["unsupported_version_lines"], 1);
    let rows = value["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .map(|r| r["calls"].as_u64().unwrap())
            .sum::<u64>(),
        3
    );
}

#[test]
fn summarize_skips_non_utf8_lines_and_continues() {
    let f = Fixture::new();
    let records = fixture_records();
    f.write_records(&records[..1]);
    let path = f.0.path().join("state/cruise/usage.ndjson");
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\xff\n");
    bytes.extend_from_slice(records[1].to_string().as_bytes());
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();

    let value = f.json(&[]);
    assert_eq!(value["malformed_lines"], 1);
    assert_eq!(value["rows"][0]["calls"], 2);
    assert_eq!(value["rows"][0]["input_tokens"], 30);
}

#[test]
fn summarize_handles_empty_and_unterminated_lines() {
    let f = Fixture::new();
    f.write_records(&fixture_records()[..1]);
    let path = f.0.path().join("state/cruise/usage.ndjson");
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\n");
    bytes.extend_from_slice(fixture_records()[1].to_string().as_bytes());
    fs::write(path, bytes).unwrap();
    let value = f.json(&[]);
    assert_eq!(value["malformed_lines"], 1);
    assert_eq!(value["rows"][0]["calls"], 2);
    assert_eq!(value["rows"][0]["input_tokens"], 30);
}

#[test]
fn summarize_propagates_read_errors() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.path().join("state/cruise/usage.ndjson")).unwrap();
    let output = f.run(&[]);
    assert!(
        !output.status.success(),
        "directory read unexpectedly succeeded"
    );
    assert!(!output.stderr.is_empty(), "read error should be reported");
}

#[test]
fn summarize_by_model_expands_by_model_entries_and_preserves_totals() {
    let f = Fixture::new();
    f.write_records(&[record("one", "build", "claude", Some("requested"), &json!({"status":"reported", "input_tokens":300, "by_model":[{"model":"haiku", "input_tokens":100}, {"model":"sonnet", "input_tokens":200}]}))]);
    let value = f.json(&["--by", "model"]);
    let rows = value["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (
            rows[0]["model"].as_str(),
            rows[0]["calls"].as_u64(),
            rows[0]["input_tokens"].as_u64()
        ),
        (Some("haiku"), Some(1), Some(100))
    );
    assert_eq!(
        (
            rows[1]["model"].as_str(),
            rows[1]["calls"].as_u64(),
            rows[1]["input_tokens"].as_u64()
        ),
        (Some("sonnet"), Some(1), Some(200))
    );
    assert_eq!(
        rows.iter()
            .map(|r| r["input_tokens"].as_u64().unwrap())
            .sum::<u64>(),
        300
    );
    let unsplit = f.json(&["--by", "step"]);
    assert_eq!(unsplit["rows"][0]["calls"], 1);
    assert_eq!(unsplit["rows"][0]["input_tokens"], 300);
}

#[test]
fn expanded_null_model_is_default_and_record_null_stays_null() {
    let f = Fixture::new();
    f.write_records(&[
        record(
            "one",
            "expanded",
            "claude",
            Some("requested"),
            &json!({"status":"reported", "by_model":[{"model":null, "input_tokens":5}]}),
        ),
        record("one", "ordinary", "command", None, &unavailable()),
    ]);
    let text = f.success(&[]);
    assert!(
        text.lines()
            .any(|line| line.starts_with("expanded  claude  (default)  ")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.starts_with("ordinary  command  -  ")),
        "{text}"
    );
    let value = f.json(&[]);
    assert_eq!(value["rows"][0]["model"], "(default)");
    assert_eq!(value["rows"][1]["model"], Value::Null);
    let csv = f.success(&["--format", "csv"]);
    assert!(
        csv.lines()
            .any(|line| line.starts_with("expanded,claude,(default),")),
        "{csv}"
    );
    assert!(
        csv.lines()
            .any(|line| line.starts_with("ordinary,command,,")),
        "{csv}"
    );
}

#[test]
fn summarize_unavailable_is_null_not_zero() {
    let f = Fixture::new();
    f.write_records(&[
        record("one", "mixed", "command", None, &unavailable()),
        record(
            "one",
            "mixed",
            "claude",
            None,
            &json!({"status":"reported", "input_tokens":10}),
        ),
        record("one", "unknown", "command", None, &unavailable()),
    ]);
    let value = f.json(&["--by", "step"]);
    let rows = value["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["input_tokens"], 10);
    assert_eq!(rows[0]["unavailable_calls"], 1);
    assert_eq!(rows[0]["reported_calls"], 1);
    assert_eq!(rows[1]["unavailable_calls"], 1);
    assert_eq!(rows[1]["reported_calls"], 0);
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "reasoning_tokens",
        "total_tokens",
        "cost_usd",
    ] {
        assert_eq!(rows[1][key], Value::Null, "{key}");
    }
}

#[test]
fn summarize_total_tokens_requires_both_input_and_output_per_record() {
    let f = Fixture::new();
    f.write_records(&[
        record("one", "build", "claude", None, &json!({"status":"reported", "input_tokens":100})),
        record("one", "build", "claude", None, &json!({"status":"reported", "input_tokens":10, "output_tokens":5, "cache_read_input_tokens":40, "cache_creation_input_tokens":30, "reasoning_tokens":20})),
        record("one", "partial", "claude", None, &json!({"status":"reported", "input_tokens":100})),
    ]);
    let value = f.json(&["--by", "step"]);
    assert_eq!(value["rows"][0]["input_tokens"], 110);
    assert_eq!(value["rows"][0]["total_tokens"], 15);
    assert_eq!(value["rows"][1]["total_tokens"], Value::Null);
}

#[test]
fn csv_escapes_comma_quote_and_newline() {
    let f = Fixture::new();
    f.write_records(&[record("one", "a,\"b\"\nc", "command", None, &unavailable())]);
    let text = f.success(&["--format", "csv"]);
    assert!(
        text.contains("\"a,\"\"b\"\"\nc\",command,,1,1,0,,,,,,,"),
        "{text:?}"
    );
}
