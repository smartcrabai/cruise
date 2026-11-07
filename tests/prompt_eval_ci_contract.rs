//! Contract for the manually invoked, opt-in live prompt evaluation workflow.
use serde_yaml::Value;
use std::{error::Error, fs, path::PathBuf};

fn workflow() -> Result<Value, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut matches = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if !matches!(
            path.extension().and_then(|x| x.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let doc: Value = serde_yaml::from_str(&fs::read_to_string(&path)?)
            .map_err(|e| format!("parse {}: {e}", path.display()))?;
        if doc["jobs"].as_mapping().is_some_and(|jobs| {
            jobs.values().any(|job| {
                job["steps"].as_sequence().is_some_and(|steps| {
                    steps.iter().any(|step| {
                        step["run"]
                            .as_str()
                            .is_some_and(|run| run.contains("--test prompt_eval"))
                    })
                })
            })
        }) {
            matches.push((path, doc));
        }
    }
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one workflow running the live prompt evaluation"
    );
    Ok(matches.pop().ok_or("missing evaluation workflow")?.1)
}

fn eval_job(doc: &Value) -> Result<&Value, Box<dyn Error>> {
    doc["jobs"]
        .as_mapping()
        .ok_or("missing jobs mapping")?
        .values()
        .find(|job| {
            job["steps"].as_sequence().is_some_and(|steps| {
                steps.iter().any(|step| {
                    step["run"]
                        .as_str()
                        .is_some_and(|run| run.contains("--test prompt_eval"))
                })
            })
        })
        .ok_or_else(|| "missing evaluation job".into())
}

#[test]
fn workflow_is_manual_and_least_privilege() -> Result<(), Box<dyn Error>> {
    let doc = workflow()?;
    let triggers = doc["on"].as_mapping().ok_or("manual trigger mapping")?;
    assert_eq!(triggers.len(), 1, "workflow must only run manually");
    assert!(triggers.contains_key(Value::String("workflow_dispatch".into())));
    let permissions = doc["permissions"]
        .as_mapping()
        .ok_or("explicit workflow permissions")?;
    assert_eq!(permissions.len(), 1, "grant only repository read access");
    assert_eq!(doc["permissions"]["contents"].as_str(), Some("read"));
    let job = eval_job(&doc)?;
    if !job["permissions"].is_null() {
        assert_eq!(
            job["permissions"]
                .as_mapping()
                .ok_or("job permissions mapping")?
                .len(),
            1
        );
        assert_eq!(job["permissions"]["contents"].as_str(), Some("read"));
    }
    let env = &job["environment"];
    assert_eq!(
        env.as_str().or_else(|| env["name"].as_str()),
        Some("prompt-eval")
    );
    Ok(())
}

#[test]
fn checkout_and_live_evaluation_are_explicit() -> Result<(), Box<dyn Error>> {
    let doc = workflow()?;
    let job = eval_job(&doc)?;
    let steps = job["steps"].as_sequence().ok_or("steps sequence")?;
    let checkout = steps
        .iter()
        .find(|s| {
            s["uses"]
                .as_str()
                .is_some_and(|u| u.starts_with("actions/checkout@"))
        })
        .ok_or("checkout step")?;
    assert_eq!(checkout["with"]["fetch-depth"].as_i64(), Some(0));
    assert_eq!(
        checkout["with"]["persist-credentials"].as_bool(),
        Some(false)
    );
    let live = steps
        .iter()
        .find(|s| {
            s["run"]
                .as_str()
                .is_some_and(|r| r.contains("--test prompt_eval"))
        })
        .ok_or("live evaluation step")?;
    let run = live["run"].as_str().ok_or("live evaluation command")?;
    assert!(
        run.contains("cargo test")
            && run.contains("--all-features")
            && run.contains("--ignored")
            && run.contains("--nocapture"),
        "run the live ignored evaluation: {run}"
    );
    let opt_in = [
        &live["env"]["CRUISE_PROMPT_EVAL"],
        &job["env"]["CRUISE_PROMPT_EVAL"],
        &doc["env"]["CRUISE_PROMPT_EVAL"],
    ];
    assert!(
        opt_in
            .iter()
            .any(|v| v.as_str() == Some("1") || v.as_i64() == Some(1)),
        "live evaluation must be explicitly opted in"
    );
    Ok(())
}

#[test]
fn evaluation_artifacts_upload_even_on_failure() -> Result<(), Box<dyn Error>> {
    let doc = workflow()?;
    let steps = eval_job(&doc)?["steps"]
        .as_sequence()
        .ok_or("steps sequence")?;
    let upload = steps
        .iter()
        .find(|s| {
            s["uses"]
                .as_str()
                .is_some_and(|u| u.starts_with("actions/upload-artifact@"))
        })
        .ok_or("upload evaluation artifacts")?;
    assert!(
        upload["if"]
            .as_str()
            .is_some_and(|condition| condition.contains("always()")),
        "artifact upload must run after failures"
    );
    assert_eq!(upload["with"]["path"].as_str(), Some("target/prompt-eval/"));
    assert_eq!(upload["with"]["name"].as_str(), Some("prompt-eval-reports"));
    assert_eq!(upload["with"]["retention-days"].as_i64(), Some(14));
    assert_eq!(upload["with"]["if-no-files-found"].as_str(), Some("warn"));
    Ok(())
}
