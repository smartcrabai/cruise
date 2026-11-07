use cruise::config::{StepConfig, WorkflowConfig};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    cases: Vec<Case>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub target: Target,
    pub render: String,
    pub output: String,
    #[serde(default)]
    pub language: Option<Language>,
    #[serde(default)]
    pub vars: BTreeMap<String, Var>,
    #[serde(default)]
    pub checks: Vec<Check>,
    #[serde(default)]
    pub reference: String,
    pub rubric: Vec<Rubric>,
    #[serde(default)]
    pub repeat: Option<usize>,
    #[serde(default)]
    pub workspace: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub prompt_file: Option<String>,
    pub workflow_step: Option<WorkflowTarget>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTarget {
    pub path: String,
    pub step: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Language {
    pub var: String,
    pub values: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Var {
    Text(String),
    File { file: String },
}
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Check {
    Contains { contains: String },
    NotContains { not_contains: String },
    Matches { matches: String },
    NotMatches { not_matches: String },
    MinChars { min_chars: usize },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rubric {
    pub id: String,
    pub text: String,
}

pub fn load() -> Result<Vec<Case>, String> {
    let dir = root().join("tests/prompt_eval/cases");
    let mut paths: Vec<_> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|x| x.path()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    paths.retain(|p| matches!(p.extension().and_then(|x| x.to_str()), Some("yaml" | "yml")));
    paths.sort();
    if paths.is_empty() {
        return Err("empty case catalog".into());
    }
    let mut cases = Vec::new();
    for p in paths {
        let text = fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let doc: Document =
            serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        cases.extend(doc.cases);
    }
    Ok(cases)
}
pub fn source(case: &Case) -> Result<&str, String> {
    match (&case.target.prompt_file, &case.target.workflow_step) {
        (Some(p), None) => Ok(p),
        (None, Some(s)) => Ok(&s.path),
        _ => Err(format!("{}: exactly one target required", case.id)),
    }
}
pub fn template(
    case: &Case,
    content: &str,
) -> Result<(String, Option<StepConfig>, WorkflowConfig), String> {
    if let Some(target) = &case.target.workflow_step {
        let config = WorkflowConfig::from_yaml(content).map_err(|e| e.to_string())?;
        let mut segments = target.step.split('/');
        let first = segments.next().ok_or("empty step")?;
        let mut step = if let Some(group) = config.groups.get(first) {
            let name = segments.next().ok_or("missing group step")?;
            group.steps.get(name)
        } else {
            config.steps.get(first)
        }
        .ok_or_else(|| format!("unknown step {}", target.step))?;
        for segment in segments {
            step = step
                .parallel
                .as_ref()
                .and_then(|p| p.get(segment))
                .ok_or_else(|| format!("unknown parallel step {}", target.step))?;
        }
        let text = step
            .prompt
            .clone()
            .ok_or_else(|| format!("step {} has no prompt", target.step))?;
        Ok((text, Some(step.clone()), config))
    } else {
        Ok((
            content.to_owned(),
            None,
            WorkflowConfig::from_yaml("steps: {}\n").map_err(|e| e.to_string())?,
        ))
    }
}
pub fn validate(cases: &[Case]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut rubrics = HashSet::new();
    let id_re = regex::Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").unwrap();
    for c in cases {
        if !id_re.is_match(&c.id) || !ids.insert(&c.id) {
            return Err(format!("invalid or duplicate case id {}", c.id));
        }
        if !matches!(
            (c.render.as_str(), c.output.as_str()),
            ("plan", "plan_file" | "text")
                | ("step", "text" | "workspace_diff")
                | ("pr", "pr_metadata")
                | ("generate", "workflow_yaml")
        ) {
            return Err(format!("{}: invalid render/output", c.id));
        }
        if c.output == "workspace_diff" && c.workspace.is_none() {
            return Err(format!("{}: missing workspace", c.id));
        }
        if c.repeat == Some(0) || c.rubric.is_empty() {
            return Err(format!("{}: empty repeat or rubric", c.id));
        }
        if let Some(l) = &c.language {
            if l.values.is_empty() || l.values.iter().any(|v| v != "English" && v != "Japanese") {
                return Err(format!("{}: invalid language", c.id));
            }
        }
        for r in &c.rubric {
            if !id_re.is_match(&r.id) || !rubrics.insert(&r.id) {
                return Err(format!("invalid or duplicate rubric {}", r.id));
            }
        }
        for check in &c.checks {
            if let Check::Matches { matches }
            | Check::NotMatches {
                not_matches: matches,
            } = check
            {
                regex::Regex::new(matches).map_err(|e| e.to_string())?;
            }
        }
        let path = source(c)?;
        let data = fs::read_to_string(root().join(path)).map_err(|e| format!("{path}: {e}"))?;
        template(c, &data)?;
        for var in c.vars.values() {
            if let Var::File { file } = var {
                if !root().join(file).is_file() {
                    return Err(format!("missing fixture {file}"));
                }
            }
        }
        if let Some(w) = &c.workspace {
            if !root().join(w).is_dir() {
                return Err(format!("missing workspace {w}"));
            }
        }
    }
    Ok(())
}
pub fn check_output(check: &Check, output: &str) -> bool {
    match check {
        Check::Contains { contains } => output.contains(contains),
        Check::NotContains { not_contains } => !output.contains(not_contains),
        Check::Matches { matches } => regex::Regex::new(matches).is_ok_and(|r| r.is_match(output)),
        Check::NotMatches { not_matches } => {
            regex::Regex::new(not_matches).is_ok_and(|r| !r.is_match(output))
        }
        Check::MinChars { min_chars } => output.chars().count() >= *min_chars,
    }
}
pub fn language_ok(lang: &str, output: &str) -> bool {
    let total = output.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        return false;
    }
    let japanese = output
        .chars()
        .filter(|c| ('\u{3040}'..='\u{30ff}').contains(c) || ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    let ratio = japanese as f64 / total as f64;
    match lang {
        "Japanese" => ratio >= 0.10,
        "English" => ratio <= 0.01,
        _ => false,
    }
}

pub fn read_source(path: &str, base: Option<&str>) -> Result<String, String> {
    read_source_at(&root(), path, base)
}

pub fn read_source_at(dir: &Path, path: &str, base: Option<&str>) -> Result<String, String> {
    if let Some(sha) = base {
        let result = std::process::Command::new("git")
            .current_dir(dir)
            .args(["show", &format!("{sha}:{path}")])
            .output()
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(String::from_utf8_lossy(&result.stderr).into_owned());
        }
        String::from_utf8(result.stdout).map_err(|e| e.to_string())
    } else {
        fs::read_to_string(dir.join(Path::new(path))).map_err(|e| e.to_string())
    }
}
