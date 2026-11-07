mod catalog;
#[cfg(test)]
mod r#assert;
#[cfg(test)]
mod contract;
#[cfg(test)]
mod grade;
mod live;
mod vocabulary;
use catalog::*;
use cruise::{
    config::{ModelSpec, PermissionMode},
    executor::{Executor, PromptRun},
    prompt_render,
    tool_policy::ToolPolicy,
    variable::VariableStore,
};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::Duration,
};

const PROMPT_EVAL_VERSION: &str = "1";
type Result<T> = std::result::Result<T, String>;

pub fn catalog_contract() {
    let cases = load().unwrap_or_else(|e| panic!("catalog: {e}"));
    validate(&cases).unwrap_or_else(|e| panic!("catalog: {e}"));
    let text =
        fs::read_to_string(root().join("tests/prompt_eval/canaries.yaml")).expect("read canaries");
    let canaries: Canaries = serde_yaml::from_str(&text).expect("parse canaries");
    let expected: HashSet<_> = cases
        .iter()
        .flat_map(|c| &c.rubric)
        .map(|r| r.id.as_str())
        .collect();
    let mut coverage = HashSet::new();
    for canary in &canaries.canaries {
        assert!(
            expected.contains(canary.rubric_id.as_str()),
            "unknown canary {}",
            canary.rubric_id
        );
        coverage.insert((canary.rubric_id.as_str(), canary.pass));
    }
    for id in expected {
        for pass in [true, false] {
            assert!(
                coverage.contains(&(id, pass)),
                "missing canary {id} expected {pass}"
            );
        }
    }
}
#[test]
fn deterministic_checks() {
    assert!(check_output(
        &Check::Contains {
            contains: "abc".into()
        },
        "xabc"
    ));
    assert!(!check_output(
        &Check::NotMatches {
            not_matches: "a+".into()
        },
        "aa"
    ));
    assert!(check_output(&Check::MinChars { min_chars: 2 }, "日本"));
    assert!(language_ok("Japanese", "日本語"));
    assert!(language_ok("English", "hello"));
    assert!(!language_ok("English", ""));
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Judgment {
    verdicts: Vec<Verdict>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Verdict {
    id: String,
    pass: bool,
    evidence: String,
}
fn parse_judgment(raw: &str, rubric: &[Rubric]) -> Result<Judgment> {
    let s = raw.trim();
    let s = if let Some(s) = s
        .strip_prefix("```json\n")
        .or_else(|| s.strip_prefix("```\n"))
    {
        s.strip_suffix("```").ok_or("unclosed code fence")?.trim()
    } else {
        s
    };
    let result: Judgment =
        serde_json::from_str(s).map_err(|e| format!("invalid judge JSON: {e}"))?;
    let expected: HashSet<_> = rubric.iter().map(|r| r.id.as_str()).collect();
    let actual: HashSet<_> = result.verdicts.iter().map(|r| r.id.as_str()).collect();
    if expected != actual || actual.len() != result.verdicts.len() {
        return Err("judge verdict IDs do not match rubric".into());
    }
    Ok(result)
}
fn record_judgment(record: &mut Trial, raw: String, rubric: &[Rubric]) -> Result<Judgment> {
    let parsed = parse_judgment(&raw, rubric);
    record.judge_raw = Some(raw);
    parsed
}
#[test]
fn strict_judge_parser() {
    let r = vec![Rubric {
        id: "one".into(),
        text: "test".into(),
    }];
    assert!(
        parse_judgment(
            "{\"verdicts\":[{\"id\":\"one\",\"pass\":true,\"evidence\":\"yes\"}]}",
            &r
        )
        .is_ok()
    );
    assert!(parse_judgment("prefix {\"verdicts\":[]}", &r).is_err());
    assert!(parse_judgment("{\"verdicts\":[]}", &r).is_err());
    assert!(
        parse_judgment(
            "```json\n{\"verdicts\":[{\"id\":\"one\",\"pass\":true,\"evidence\":\"ok\"}]}\n```",
            &r
        )
        .is_ok()
    );
    assert!(
        parse_judgment(
            "{\"verdicts\":[{\"id\":\"one\",\"pass\":\"true\",\"evidence\":\"ok\"}]}",
            &r
        )
        .is_err()
    );
    assert!(parse_judgment("{\"verdicts\":[{\"id\":\"one\",\"pass\":true,\"evidence\":\"ok\"},{\"id\":\"one\",\"pass\":true,\"evidence\":\"ok\"}]}", &r).is_err());
}
fn git(args: &[&str], dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
fn hash(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn template_hashes(dir: &Path, rows: &[Row], base: Option<&str>, hashes: &mut BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let mut errors = BTreeMap::new();
    for row in rows {
        let case = &row.case;
        let path = source(case)?;
        let data = read_source_at(dir, path, None)?;
        let (text, _, _) = template(case, &data)?;
        hashes.insert(format!("{}:{}", path, case.id), hash(text.as_bytes()));
        if let Some(base) = base {
            match read_source_at(dir, path, Some(base)).and_then(|data| template(case, &data)) {
                Ok((text, _, _)) => { hashes.insert(format!("base:{}:{}", path, case.id), hash(text.as_bytes())); }
                Err(error) => { errors.insert(row.id.clone(), error); }
            }
        }
    }
    Ok(errors)
}
fn number(name: &str, default: usize) -> Result<usize> {
    let value = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let n = value
        .parse::<usize>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if n == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(n)
}
fn required(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err("fixture symlinks are unsupported".into());
        }
        if ty.is_dir() {
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn setup_workspace(c: &Case, work: &Path) -> Result<()> {
    if let Some(w) = &c.workspace {
        copy_tree(&root().join(w), work)?;
        if c.id == "create-pr" {
            fs::remove_file(work.join("tests/add.rs")).map_err(|e| e.to_string())?;
        }
        git(&["init", "-q"], work)?;
        git(&["add", "-A"], work)?;
        fixture_commit(work, "fixture")?;
        if c.id == "create-pr" {
            fs::copy(root().join("tests/prompt_eval/fixtures/addition/src/lib.rs"), work.join("src/lib.rs")).map_err(|e| e.to_string())?;
            git(&["add", "src/lib.rs"], work)?;
            fixture_commit(work, "Implement checked addition")?;
            fs::create_dir_all(work.join("tests")).map_err(|e| e.to_string())?;
            fs::copy(root().join("tests/prompt_eval/fixtures/addition-unimplemented/tests/add.rs"), work.join("tests/add.rs")).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn fixture_commit(work: &Path, message: &str) -> Result<()> {
    git(&["-c", "user.name=prompt-eval", "-c", "user.email=prompt-eval@invalid", "commit", "-q", "--allow-empty", "-m", message], work)?;
    Ok(())
}
#[test]
fn create_pr_fixture_contract() {
    let cases = load().unwrap();
    let case = cases.iter().find(|c| c.id == "create-pr").unwrap();
    let dir = tempfile::tempdir().unwrap();
    setup_workspace(case, dir.path()).unwrap();
    let work = dir.path();
    let committed = git(&["diff", "HEAD~1..HEAD"], work).unwrap();
    assert!(committed.contains("src/lib.rs") && committed.contains("checked_add"));
    let status = git(&["status", "--short"], work).unwrap();
    assert!(status.contains("tests/"));
    let tests = fs::read_to_string(work.join("tests/add.rs")).unwrap();
    for value in ["add(2, 3)", "add(-2, 1)", "add(0, 0)", "i32::MAX"] { assert!(tests.contains(value)); }
    for claim in ["committed", "checked addition", "src/lib.rs", "uncommitted", "tests/add.rs", "positive", "negative", "zero", "overflow"] { assert!(case.reference.contains(claim)); }
}
async fn execute(
    sdk: &str,
    model: &str,
    prompt: &str,
    dir: &Path,
    policy: ToolPolicy,
    permission: PermissionMode,
    timeout: usize,
) -> Result<(String, Vec<String>)> {
    let executor = Executor::new(Some(sdk), &[]);
    let model = ModelSpec::Single(model.into());
    let env = Default::default();
    let mcp = Default::default();
    let notices = Mutex::new(Vec::new());
    let notice = |s: &str| notices.lock().unwrap().push(s.to_owned());
    let run = executor.run(PromptRun {
        prompt,
        model: Some(&model),
        subagent_model: None,
        max_retries: 2,
        env: &env,
        mcp_servers: &mcp,
        on_notice: Some(&notice),
        cancel_token: None,
        working_dir: Some(dir),
        stream: None,
        tools: Vec::new(),
        on_session_id: None,
        resume: None,
        permission,
        tool_policy: policy,
        computer_use: false,
        usage: None,
    });
    let result = tokio::time::timeout(Duration::from_secs(timeout as u64), run)
        .await
        .map_err(|_| "trial timeout".to_owned())?
        .map_err(|e| e.to_string())?;
    Ok((result.result.output, notices.into_inner().unwrap()))
}
fn judge_prompt(
    template: &str,
    rubric: &[Rubric],
    reference: &str,
    output: &str,
) -> Result<String> {
    Ok(format!(
        "{template}\n\n## Rubric\n{}\n\n## Reference\n{reference}\n\n## Candidate output\n{output}",
        serde_json::to_string(rubric).map_err(|e| e.to_string())?
    ))
}
#[derive(Deserialize)]
struct Canaries {
    canaries: Vec<Canary>,
}
#[derive(Deserialize)]
struct Canary {
    #[serde(alias = "id")]
    rubric_id: String,
    #[serde(alias = "expected")]
    pass: bool,
    #[serde(alias = "candidate")]
    output: String,
    #[serde(default)]
    reference: String,
}
async fn calibrate(
    cases: &[Case],
    sdk: &str,
    judge: &str,
    judge_template: &str,
    timeout: usize,
    report: &Path,
) -> Result<()> {
    let text = fs::read_to_string(root().join("tests/prompt_eval/canaries.yaml"))
        .map_err(|e| e.to_string())?;
    let doc: Canaries = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    let mut covered = HashSet::new();
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    for c in doc.canaries {
        let r = cases
            .iter()
            .flat_map(|c| &c.rubric)
            .find(|r| r.id == c.rubric_id)
            .ok_or_else(|| format!("unknown canary rubric {}", c.rubric_id))?;
        covered.insert((c.rubric_id.clone(), c.pass));
        let rubric = std::slice::from_ref(r);
        let prompt = judge_prompt(judge_template, rubric, &c.reference, &c.output)?;
        let raw = execute(
            sdk,
            judge,
            &prompt,
            temp.path(),
            ToolPolicy::default(),
            PermissionMode::ReadOnly,
            timeout,
        )
        .await
        .map(|x| x.0);
        let parsed = raw
            .as_ref()
            .ok()
            .and_then(|v| parse_judgment(v, rubric).ok())
            .and_then(|v| v.verdicts.first().map(|v| v.pass));
        results.push(serde_json::json!({"rubric_id":c.rubric_id,"expected":c.pass,"actual":parsed,"raw":raw.as_ref().ok(),"error":raw.as_ref().err()}));
    }
    fs::write(
        report.join("canaries.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    for r in cases.iter().flat_map(|c| &c.rubric) {
        for expected in [true, false] {
            if !covered.contains(&(r.id.clone(), expected)) {
                return Err(format!("missing canary {} {expected}", r.id));
            }
        }
    }
    if results.iter().any(|v| v["actual"] != v["expected"]) {
        return Err("canary calibration failed".into());
    }
    Ok(())
}
#[derive(Clone)]
struct Row {
    case: Case,
    id: String,
    language: Option<String>,
}
fn select_rows(cases: &[Case], filter: Option<&regex::Regex>) -> Vec<Row> {
    let mut rows = Vec::new();
    for case in cases {
        let languages: Vec<Option<String>> = case.language.as_ref()
            .map(|l| l.values.iter().cloned().map(Some).collect())
            .unwrap_or_else(|| vec![None]);
        for language in languages {
            let id = language.as_ref()
                .map_or_else(|| case.id.clone(), |l| format!("{}@{l}", case.id));
            if filter.is_none_or(|r| r.is_match(&id)) {
                rows.push(Row { case: case.clone(), id, language });
            }
        }
    }
    rows
}
#[tokio::test]
async fn comparison_hashes_selected_rows_with_missing_base() {
    let cases = load().unwrap();
    let mut prompts = cases.iter().filter(|c| c.target.prompt_file.is_some() && c.language.is_none());
    let valid = prompts.next().unwrap().clone();
    let mut missing = prompts.find(|c| source(c).unwrap() != source(&valid).unwrap()).unwrap().clone();
    missing.workspace = None;
    let dir = tempfile::tempdir().unwrap();
    git(&["init", "-q"], dir.path()).unwrap();
    for case in [&valid, &missing] {
        let path = source(case).unwrap();
        let dest = dir.path().join(path);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::copy(root().join(path), dest).unwrap();
    }
    git(&["add", source(&valid).unwrap()], dir.path()).unwrap();
    fixture_commit(dir.path(), "base").unwrap();
    let filter = regex::Regex::new(&format!("^({}|{})$", valid.id, missing.id)).unwrap();
    let rows = select_rows(&[valid.clone(), missing.clone()], Some(&filter));
    let mut hashes = BTreeMap::new();
    let errors = template_hashes(dir.path(), &rows, Some("HEAD"), &mut hashes).unwrap();
    assert!(hashes.contains_key(&format!("base:{}:{}", source(&valid).unwrap(), valid.id)));
    assert!(!hashes.contains_key(&format!("base:{}:{}", source(&missing).unwrap(), missing.id)));
    assert!(hashes.contains_key(&format!("{}:{}", source(&missing).unwrap(), missing.id)));
    let filter = regex::Regex::new(&format!("^{}$", valid.id)).unwrap();
    let selected = select_rows(&[valid.clone(), missing.clone()], Some(&filter));
    let mut filtered = BTreeMap::new();
    template_hashes(dir.path(), &selected, Some("HEAD"), &mut filtered).unwrap();
    assert_eq!(filtered.len(), 2);
    assert!(!filtered.contains_key(&format!("{}:{}", source(&missing).unwrap(), missing.id)));
    let manifest = serde_json::json!({"hashes": hashes});
    fs::write(dir.path().join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let settings = Settings {
        sdk: "jcode".into(), candidate: "candidate".into(), judge: "judge".into(),
        timeout: 30, judge_template: String::new(), report: dir.path().to_path_buf(),
        source_root: dir.path().to_path_buf(),
    };
    let missing_row = rows.iter().find(|r| r.id == missing.id).unwrap().clone();
    let base_error = trial(missing_row, "base", Some("HEAD"), 1, &settings, errors.get(&missing.id)).await;
    assert_eq!(base_error.status, "error");
    assert!(base_error.error.as_ref().is_some_and(|e| !e.is_empty()));
    // Successful trials are represented explicitly so this contract needs no model provider.
    let passed = |row: &str, side: &str| Trial {
        side: side.into(), row: row.into(), trial: 1, status: "pass".into(),
        checks: vec![], rubric: serde_json::json!([]), judge_raw: None,
        notices: vec![], error: None, output_path: None, output_source: None,
    };
    let results = vec![passed(&missing.id, "candidate"), base_error,
        passed(&valid.id, "candidate"), passed(&valid.id, "base")];
    let failed = write_results(dir.path(), &results).unwrap();
    assert!(failed.is_empty());
    let records: Vec<serde_json::Value> = fs::read_to_string(dir.path().join("results.jsonl"))
        .unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(records.len(), 4);
    assert!(records.iter().any(|r| r["row"] == missing.id && r["side"] == "base" && r["status"] == "error" && r["error"].is_string()));
    let summary = fs::read_to_string(dir.path().join("summary.md")).unwrap();
    assert!(summary.contains(&missing.id));
    assert!(summary.contains(&valid.id));
    assert!(summary.contains("base error"));
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(dir.path().join("manifest.json")).unwrap()).unwrap();
    assert!(manifest["hashes"][format!("base:{}:{}", source(&valid).unwrap(), valid.id)].is_string());
}
#[derive(Serialize)]
struct Trial {
    side: String,
    row: String,
    trial: usize,
    status: String,
    checks: Vec<serde_json::Value>,
    rubric: serde_json::Value,
    judge_raw: Option<String>,
    notices: Vec<String>,
    error: Option<String>,
    output_path: Option<String>,
    output_source: Option<String>,
}
struct Settings {
    sdk: String,
    candidate: String,
    judge: String,
    timeout: usize,
    judge_template: String,
    report: PathBuf,
    source_root: PathBuf,
}
async fn trial(row: Row, side: &str, base: Option<&str>, n: usize, settings: &Settings, base_error: Option<&String>) -> Trial {
    let mut record = Trial {
        side: side.into(),
        row: row.id.clone(),
        trial: n,
        status: "error".into(),
        checks: vec![],
        rubric: serde_json::json!("skipped"),
        judge_raw: None,
        notices: vec![],
        error: None,
        output_path: None,
        output_source: None,
    };
    if let Some(error) = base_error {
        record.error = Some(error.clone());
    } else if let Err(e) = trial_deadline(Duration::from_secs(settings.timeout as u64), trial_inner(&row, base, settings, &mut record)).await {
        record.error = Some(e);
    }
    record
}

async fn trial_deadline<T>(duration: Duration, work: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(duration, work).await.map_err(|_| "trial timeout".to_owned())?
}

#[tokio::test]
async fn deadline_covers_sequential_candidate_and_judge() {
    let result = trial_deadline(Duration::from_millis(100), async {
        tokio::time::sleep(Duration::from_millis(60)).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        Ok(())
    }).await;
    assert_eq!(result.unwrap_err(), "trial timeout");
}
pub async fn run() -> Result<()> {
    let sdk = std::env::var("CRUISE_PROMPT_EVAL_SDK").unwrap_or_else(|_| "jcode".into());
    if !matches!(sdk.as_str(), "jcode" | "claude" | "opencode") {
        return Err(format!("unsupported sdk {sdk}"));
    }
    let candidate = required("CRUISE_PROMPT_EVAL_MODEL")?;
    let judge = required("CRUISE_PROMPT_EVAL_JUDGE_MODEL")?;
    if candidate == judge {
        return Err("candidate and judge models must differ".into());
    }
    let repeat = number("CRUISE_PROMPT_EVAL_REPEAT", 3)?;
    let concurrency = number("CRUISE_PROMPT_EVAL_CONCURRENCY", 3)?;
    let timeout = number("CRUISE_PROMPT_EVAL_TRIAL_TIMEOUT", 900)?;
    let filter = std::env::var("CRUISE_PROMPT_EVAL_CASE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| regex::Regex::new(&s).map_err(|e| e.to_string()))
        .transpose()?;
    let base_ref = std::env::var("CRUISE_PROMPT_EVAL_BASE")
        .ok()
        .filter(|s| !s.is_empty());
    let base = base_ref
        .as_ref()
        .map(|r| {
            git(
                &["rev-parse", "--verify", &format!("{r}^{{commit}}")],
                &root(),
            )
            .map(|s| s.trim().to_owned())
        })
        .transpose()?;
    let sha = git(&["rev-parse", "HEAD"], &root())?.trim().to_owned();
    let dirty = !git(&["status", "--porcelain"], &root())?.trim().is_empty();
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let stamp = Command::new("date")
        .args(["-u", "+%Y%m%dT%H%M%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| started.to_string());
    let output_root = std::env::var("CRUISE_PROMPT_EVAL_OUTPUT").ok();
    let report = live::output_root(&root(), output_root.as_deref()).join(format!(
        "{}-{}{}",
        stamp,
        &sha[..sha.len().min(8)],
        if dirty { "-dirty" } else { "" }
    ));
    fs::create_dir_all(&report).map_err(|e| e.to_string())?;
    let cases = load()?;
    validate(&cases)?;
    let rows = select_rows(&cases, filter.as_ref());
    let judge_template =
        fs::read_to_string(root().join("tests/prompt_eval/judge.md")).map_err(|e| e.to_string())?;
    let mut hashes = BTreeMap::new();
    for entry in fs::read_dir(root().join("tests/prompt_eval/cases")).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file() {
            hashes.insert(
                p.to_string_lossy().into_owned(),
                hash(&fs::read(&p).map_err(|e| e.to_string())?),
            );
        }
    }
    let base_errors = template_hashes(&root(), &rows, base.as_deref(), &mut hashes)?;
    hashes.insert("judge.md".into(), hash(judge_template.as_bytes()));
    let mut manifest = serde_json::json!({"version":PROMPT_EVAL_VERSION,"git_sha":sha,"dirty":dirty,"sdk":sdk,"model":candidate,"judge_model":judge,"repeat":repeat,"concurrency":concurrency,"trial_timeout":timeout,"case_filter":std::env::var("CRUISE_PROMPT_EVAL_CASE").ok(),"base_ref":base_ref,"base_sha":base,"started_unix":started,"hashes":hashes});
    manifest["output_root"] =
        serde_json::json!(report.parent().map(|p| p.to_string_lossy().into_owned()));
    let write_manifest = |m: &serde_json::Value| -> Result<()> {
        fs::write(
            report.join("manifest.json"),
            serde_json::to_vec_pretty(m).unwrap(),
        )
        .map_err(|e| e.to_string())
    };
    write_manifest(&manifest)?;
    if let Err(e) = calibrate(&cases, &sdk, &judge, &judge_template, timeout, &report).await {
        return Err(format!("{e}; artifacts: {}", report.display()));
    }
    if rows.is_empty() {
        return Err(format!(
            "no rows match filter; artifacts: {}",
            report.display()
        ));
    }
    let settings = Settings {
        sdk,
        candidate,
        judge,
        timeout,
        judge_template,
        report: report.clone(),
        source_root: root(),
    };
    let mut jobs = Vec::new();
    for row in rows {
        for n in 1..=row.case.repeat.unwrap_or(repeat) {
            jobs.push((row.clone(), "candidate", n));
            if base.is_some() {
                jobs.push((row.clone(), "base", n));
            }
        }
    }
    let results: Vec<Trial> = stream::iter(jobs.into_iter().map(|(row, side, n)| {
        let settings = &settings;
        let sha = if side == "base" {
            base.as_deref()
        } else {
            None
        };
        let error = if side == "base" { base_errors.get(&row.id) } else { None };
        async move { trial(row, side, sha, n, settings, error).await }
    }))
    .buffer_unordered(concurrency)
    .collect()
    .await;
    let failed = write_results(&report, &results)?;
    manifest["finished_unix"] = serde_json::json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    write_manifest(&manifest)?;
    println!("{}", report.join("summary.md").display());
    if !failed.is_empty() {
        return Err(format!(
            "failed rows: {}; artifacts: {}",
            failed.join(", "),
            report.display()
        ));
    }
    Ok(())
}

fn write_results(report: &Path, results: &[Trial]) -> Result<Vec<String>> {
    let mut jsonl = String::new();
    let mut counts: BTreeMap<String, BTreeMap<String, (usize, usize, bool)>> = BTreeMap::new();
    for t in results {
        jsonl.push_str(&serde_json::to_string(t).unwrap());
        jsonl.push('\n');
        let count = counts
            .entry(t.row.clone())
            .or_default()
            .entry(t.side.clone())
            .or_default();
        count.1 += 1;
        count.2 |= t.status == "error";
        if t.status == "pass" {
            count.0 += 1;
        }
    }
    fs::write(report.join("results.jsonl"), jsonl).map_err(|e| e.to_string())?;
    let (summary, failed) = live::summary(&counts);
    fs::write(report.join("summary.md"), summary).map_err(|e| e.to_string())?;
    Ok(failed)
}

async fn trial_inner(
    row: &Row,
    base: Option<&str>,
    s: &Settings,
    record: &mut Trial,
) -> Result<()> {
    let c = &row.case;
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let work = dir.path();
    setup_workspace(c, work)?;
    let candidate_content = read_source_at(&s.source_root, source(c)?, None)?;
    let (candidate_template, step, mut config) = template(c, &candidate_content)?;
    let template = if let Some(sha) = base {
        let content = read_source_at(&s.source_root, source(c)?, Some(sha))?;
        template(c, &content)?.0
    } else {
        candidate_template
    };
    let mut vars = VariableStore::new(String::new());
    vars.set_artifacts_root(work.to_path_buf());
    for (name, value) in &c.vars {
        match value {
            Var::Text(value) => {
                if name == "input" {
                    vars.set_input(value.clone());
                } else {
                    vars.set_named_value(name, value.clone());
                }
            }
            Var::File { file } => vars.set_named_file(name, root().join(file)),
        }
    }
    if let (Some(language), Some(value)) = (&c.language, &row.language) {
        vars.set_named_value(&language.var, value.clone());
        if language.var == "plan.language" {
            config.plan_language = Some(value.clone());
        }
    }
    let compiled = cruise::workflow::compile(config.clone()).map_err(|e| e.to_string())?;
    let permission = step
        .as_ref()
        .and_then(|x| x.permission)
        .unwrap_or(compiled.permission);
    let allow_commit = step.as_ref().is_some_and(|x| x.allow_commit);
    let policy = ToolPolicy::for_prompt_step(
        &compiled.disallowed_tools,
        step.as_ref().map_or(&[], |s| s.disallowed_tools.as_slice()),
        allow_commit,
        permission,
    );
    let plan_path = work.join("plan.md");
    let prompt = match c.render.as_str() {
        "plan" => {
            prompt_render::render_plan_template(&template, &mut vars, &config, &plan_path, false)
                .map_err(|e| e.to_string())?
        }
        "step" => prompt_render::assemble_step_prompt(&template, &vars, allow_commit, &policy)
            .map_err(|e| e.to_string())?,
        "pr" => prompt_render::render_pr_template(
            &template,
            &mut vars,
            row.language.as_deref().unwrap_or("English"),
        )
        .map_err(|e| e.to_string())?,
        "generate" => {
            let description = match c.vars.get("description") {
                Some(Var::Text(s)) => s.as_str(),
                _ => "",
            };
            prompt_render::render_generate_prompt(
                &template,
                include_str!("../../cruise-schema.json"),
                description,
            )
        }
        _ => return Err("invalid renderer".into()),
    };
    let artifact = s
        .report
        .join("trials")
        .join(&record.side)
        .join(&record.row)
        .join(record.trial.to_string());
    fs::create_dir_all(&artifact).map_err(|e| e.to_string())?;
    fs::write(artifact.join("prompt.md"), &prompt).map_err(|e| e.to_string())?;
    let (stdout, notices) = execute(
        &s.sdk,
        &s.candidate,
        &prompt,
        work,
        policy,
        permission,
        s.timeout,
    )
    .await?;
    record.notices = notices;
    let output = match c.output.as_str() {
        "plan_file" => plan_output(&plan_path, stdout, &mut record.output_source),
        "workspace_diff" => {
            workspace_diff(work)?
        }
        _ => stdout,
    };
    let filename = if c.output == "workspace_diff" {
        "workspace.diff"
    } else {
        "output.md"
    };
    let path = artifact.join(filename);
    fs::write(&path, &output).map_err(|e| e.to_string())?;
    record.output_path = Some(path.to_string_lossy().into_owned());
    for check in &c.checks {
        record.checks.push(
            serde_json::json!({"check":format!("{check:?}"),"pass":check_output(check,&output)}),
        );
    }
    if let Some(lang) = &row.language {
        record
            .checks
            .push(serde_json::json!({"check":"language","pass":language_ok(lang,&output)}));
    }
    if c.output == "pr_metadata" {
        let (title, body) = cruise::worktree_pr::parse_pr_metadata(&output);
        record.checks.push(serde_json::json!({"check":"pr_metadata","pass":!title.trim().is_empty() && !body.trim().is_empty()}));
    }
    if c.output == "workflow_yaml" {
        let validation = cruise::workflow_generate::validate_generated_yaml(&output, work);
        record.checks.push(serde_json::json!({"check":"workflow_yaml","pass":validation.is_ok(),"error":validation.err().map(|e|e.to_string())}));
    }
    if record.checks.iter().any(|v| v["pass"] == false) {
        record.status = "fail".into();
        return Ok(());
    }
    let judge_prompt = judge_prompt(&s.judge_template, &c.rubric, &c.reference, &output)?;
    let (raw, notices) = execute(
        &s.sdk,
        &s.judge,
        &judge_prompt,
        work,
        ToolPolicy::default(),
        PermissionMode::ReadOnly,
        s.timeout,
    )
    .await?;
    record.notices.extend(notices);
    fs::write(artifact.join("judge.json"), &raw).map_err(|e| e.to_string())?;
    let verdict = record_judgment(record, raw, &c.rubric)?;
    record.status = if verdict.verdicts.iter().all(|v| v.pass) {
        "pass"
    } else {
        "fail"
    }
    .into();
    record.rubric = serde_json::to_value(verdict.verdicts).unwrap();
    Ok(())
}

fn workspace_diff(work: &Path) -> Result<String> {
    let status = git(&["status", "--porcelain"], work)?;
    if status.trim().is_empty() {
        return Ok("no-change".into());
    }
    git(&["add", "--intent-to-add", "--all"], work)?;
    Ok(format!("{}\n{}", status, git(&["diff", "--no-ext-diff", "HEAD"], work)?))
}

fn plan_output(path: &Path, stdout: String, source: &mut Option<String>) -> String {
    match fs::read_to_string(path) {
        Ok(content) => {
            *source = Some("plan_file".into());
            content
        }
        Err(_) => {
            *source = Some("stdout".into());
            stdout
        }
    }
}

#[test]
fn missing_plan_falls_back_to_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = None;
    assert_eq!(plan_output(&dir.path().join("missing"), "candidate output".into(), &mut source), "candidate output");
    assert_eq!(source.as_deref(), Some("stdout"));
}

#[test]
fn git_base_and_workspace_diff_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(&["init", "-q"], p).unwrap();
    fs::write(p.join("prompt.md"), "old").unwrap();
    git(&["add", "-A"], p).unwrap();
    git(&["-c", "user.name=test", "-c", "user.email=test@example.invalid", "commit", "-qm", "first"], p).unwrap();
    let base = git(&["rev-parse", "HEAD"], p).unwrap();
    fs::write(p.join("prompt.md"), "new").unwrap();
    git(&["add", "-A"], p).unwrap();
    git(&["-c", "user.name=test", "-c", "user.email=test@example.invalid", "commit", "-qm", "second"], p).unwrap();
    assert_eq!(read_source_at(p, "prompt.md", Some(base.trim())).unwrap(), "old");
    assert_eq!(read_source_at(p, "prompt.md", None).unwrap(), "new");
    let mut case = load().unwrap().into_iter().find(|c| c.id == "create-pr").unwrap();
    case.target.prompt_file = Some("prompt.md".into());
    let mut hashes = BTreeMap::new();
    let rows = select_rows(&[case], None);
    template_hashes(p, &rows, Some(base.trim()), &mut hashes).unwrap();
    let manifest = serde_json::json!({"hashes": hashes});
    assert_eq!(manifest["hashes"]["prompt.md:create-pr"], hash(b"new"));
    assert_eq!(manifest["hashes"]["base:prompt.md:create-pr"], hash(b"old"));
    assert!(read_source_at(p, "prompt.md", Some("invalid-ref")).is_err());
    assert_eq!(workspace_diff(p).unwrap(), "no-change");
    fs::write(p.join("added.txt"), "added content\n").unwrap();
    let diff = workspace_diff(p).unwrap();
    assert!(diff.contains("added.txt") && diff.contains("+added content"));
}

#[test]
fn malformed_judgment_is_retained() {
    let mut record = Trial { side: "candidate".into(), row: "test".into(), trial: 0, status: "error".into(), checks: vec![], rubric: serde_json::json!("skipped"), judge_raw: None, notices: vec![], error: None, output_path: None, output_source: None };
    assert!(record_judgment(&mut record, "not json".into(), &[]).is_err());
    assert_eq!(record.judge_raw.as_deref(), Some("not json"));
}

#[test]
fn step_context_preserves_instruction_and_feedback_order() {
    let vars = VariableStore::new(String::new());
    let prompt = prompt_render::assemble_step_prompt_with_context(
        "task", &vars, false, &ToolPolicy::default(), Some("delivered"), Some("feedback"),
    ).unwrap();
    assert_eq!(prompt, format!("task\n\ndelivered{}\n\nfeedback", prompt_render::COMMIT_GUARD_PROMPT_NOTICE));
}

#[test]
fn implement_case_renders_literal_examples() {
    let cases = load().unwrap();
    let c = cases.iter().find(|c| c.id == "implement-after-tests-file").unwrap();
    let (text, _, _) = template(c, &read_source(source(c).unwrap(), None).unwrap()).unwrap();
    let mut vars = VariableStore::new(String::new());
    let Var::File { file } = &c.vars["plan"] else { panic!("expected plan file") };
    vars.set_named_file("plan", root().join(file));
    let rendered = prompt_render::assemble_step_prompt(&text, &vars, false, &ToolPolicy::default()).unwrap();
    for example in ["{Summary of work performed}", "{Summary of changes made}", "{Build execution results}", "{Test execution command and results}"] {
        assert!(rendered.contains(example));
    }
}
