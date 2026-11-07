//! Independent, offline checks of the planned prompt-evaluation catalog.
use serde_yaml::Value;
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cases() -> Result<Vec<Value>, Box<dyn Error>> {
    let dir = root().join("tests/prompt_eval/cases");
    let mut files: Vec<_> = fs::read_dir(&dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.retain(|p| matches!(p.extension().and_then(|s| s.to_str()), Some("yaml" | "yml")));
    files.sort();
    assert!(
        !files.is_empty(),
        "catalog has no YAML files in {}",
        dir.display()
    );
    let mut cases = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path)?;
        let doc: Value = serde_yaml::from_str(&text)
            .map_err(|e| format!("invalid YAML in {}: {e}", path.display()))?;
        let entries = doc
            .get("cases")
            .and_then(Value::as_sequence)
            .ok_or_else(|| format!("{} must have a cases sequence", path.display()))?;
        cases.extend(entries.iter().cloned());
    }
    Ok(cases)
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, Box<dyn Error>> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key} in {value:?}").into())
}

#[test]
fn catalog_covers_all_prompt_files() -> Result<(), Box<dyn Error>> {
    let cases = cases()?;
    let targets: HashSet<_> = cases
        .iter()
        .filter_map(|case| case.get("target")?.get("prompt_file")?.as_str())
        .collect();
    let prompt_dir = root().join("prompts");
    let files = fs::read_dir(prompt_dir)?;
    for entry in files {
        let path = entry?.path();
        if path.extension().and_then(|x| x.to_str()) == Some("md") {
            let relative = format!(
                "prompts/{}",
                path.file_name()
                    .ok_or("missing filename")?
                    .to_string_lossy()
            );
            assert!(
                targets.contains(relative.as_str()),
                "catalog does not cover {relative}"
            );
        }
    }
    Ok(())
}

#[test]
fn catalog_covers_representative_workflow_steps() -> Result<(), Box<dyn Error>> {
    let cases = cases()?;
    let mut steps: HashMap<&str, HashSet<&str>> = HashMap::new();
    for case in &cases {
        if let Some(target) = case.get("target").and_then(|v| v.get("workflow_step")) {
            let path = string(target, "path")?;
            let step = string(target, "step")?;
            assert!(
                root().join(Path::new(path)).is_file(),
                "workflow target does not exist: {path}"
            );
            steps.entry(path).or_default().insert(step);
        }
    }
    for (path, minimum) in [
        ("builtin/tdd.yaml", 4),
        ("builtin/refactor.yaml", 2),
        ("builtin/add-tests.yaml", 1),
        ("builtin/add-formal-specs.yaml", 1),
        ("builtin/research.yaml", 2),
        ("builtin/security.yaml", 2),
    ] {
        let actual = steps.get(path).map_or(0, HashSet::len);
        assert!(
            actual >= minimum,
            "{path} has {actual} distinct targets, expected at least {minimum}"
        );
    }
    Ok(())
}

#[test]
fn rubric_ids_are_unique_across_catalog() -> Result<(), Box<dyn Error>> {
    let cases = cases()?;
    let mut ids = HashSet::new();
    for case in &cases {
        let case_id = string(case, "id")?;
        let rubric = case
            .get("rubric")
            .and_then(Value::as_sequence)
            .ok_or_else(|| format!("case {case_id} has no rubric sequence"))?;
        assert!(!rubric.is_empty(), "case {case_id} has empty rubric");
        for item in rubric {
            let id = string(item, "id")?;
            assert!(ids.insert(id), "duplicate rubric id {id} in case {case_id}");
        }
    }
    Ok(())
}
