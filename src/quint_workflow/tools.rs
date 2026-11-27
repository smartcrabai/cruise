use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::artifacts;
use crate::cancellation::CancellationToken;
use crate::error::{CruiseError, Result};
use crate::formal_verify::process::run_bounded;

use super::{MAX_PNG, MODEL_LIMIT, error};

const RENDERER: &str = include_str!("../../scripts/formal/render_quint.cjs");
const PACKAGES: [(&str, &str); 3] = [
    ("@informalsystems/quint", "0.33.0"),
    ("@viz-js/viz", "3.31.0"),
    ("@resvg/resvg-js", "2.6.2"),
];

pub(crate) struct Tools(PathBuf);

impl Tools {
    pub(crate) fn load() -> Result<Self> {
        let dir = crate::paths::data_dir()?.join("quint-tools");
        for (package, version) in PACKAGES {
            let manifest = dir.join("node_modules").join(package).join("package.json");
            let installed = std::fs::read(&manifest)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            if !installed
                .as_ref()
                .is_some_and(|value| value["version"] == version)
            {
                return Err(error(format!(
                    "Install the pinned Quint tools on the daemon host: npm install --prefix \"{}\" --save-exact @informalsystems/quint@0.33.0 @viz-js/viz@3.31.0 @resvg/resvg-js@2.6.2",
                    dir.display(),
                )));
            }
        }
        Ok(Self(dir.canonicalize()?))
    }
}

pub(super) struct Checked {
    pub(super) png: Option<Vec<u8>>,
    pub(super) tests: usize,
    pub(super) log: String,
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(
    dir: &Path,
    script: &Path,
    args: &[&str],
    token: &CancellationToken,
    stage: &str,
) -> Result<String> {
    let mut command = Command::new("node");
    command
        .args(["--max-old-space-size=256", "--no-warnings"])
        .arg(script)
        .args(args)
        .current_dir(dir)
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .env("NO_COLOR", "1");
    let outcome = run_bounded(command, Duration::from_secs(60), Some(token), dir, stage);
    if outcome.cancelled || token.is_cancelled() {
        return Err(CruiseError::Interrupted);
    }
    if outcome.timed_out {
        return Err(error(format!("{stage} timed out")));
    }
    let read_log = |stream: &str| {
        let bytes = artifacts::read_bytes(dir, &format!("{stage}.{stream}.log"), 256 * 1024)
            .unwrap_or_default();
        crate::telemetry::stderr::sanitize(&String::from_utf8_lossy(&bytes)).body
    };
    let stdout = read_log("stdout");
    let stderr = read_log("stderr");
    if outcome.exit != Some(0) {
        let detail = format!(
            "{stage} failed ({:?}): {}\n{stdout}{stderr}",
            outcome.exit, outcome.message
        );
        return Err(
            if matches!(stage, "typecheck" | "test" | "render") && outcome.exit == Some(1) {
                CruiseError::CommandError(detail)
            } else {
                error(detail)
            },
        );
    }
    Ok(format!("{stdout}{stderr}"))
}

pub(super) fn self_contained(source: &[u8]) -> bool {
    let mut cursor = 0;
    while let Some(&byte) = source.get(cursor) {
        match (byte, source.get(cursor + 1).copied()) {
            (b'/', Some(b'/')) => {
                cursor += source[cursor..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .unwrap_or(source.len() - cursor);
            }
            (b'/', Some(b'*')) => {
                let tail = cursor + 2;
                cursor = source[tail..]
                    .windows(2)
                    .position(|p| p == b"*/")
                    .map_or(source.len(), |end| tail + end + 2);
            }
            (b'"', _) => {
                cursor += 1;
                while let Some(&part) = source.get(cursor) {
                    cursor += 1;
                    if part == b'"' {
                        break;
                    }
                    if part == b'\\' {
                        cursor += 1;
                    }
                }
            }
            (b, _) if b.is_ascii_alphabetic() || b == b'_' => {
                let start = cursor;
                cursor += 1;
                while source
                    .get(cursor)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    cursor += 1;
                }
                if &source[start..cursor] == b"import" {
                    return false;
                }
            }
            _ => cursor += 1,
        }
    }
    true
}

pub(super) async fn check(
    tools: &Tools,
    root: &Path,
    source: Vec<u8>,
    render: bool,
    token: &CancellationToken,
) -> Result<Checked> {
    if !self_contained(&source) {
        return Err(CruiseError::CommandError(
            "Quint review requires a self-contained model without imports".into(),
        ));
    }
    let tools = tools.0.clone();
    let root = root.to_owned();
    let token = token.clone();
    tokio::task::spawn_blocking(move || {
        let name = format!("quint/.scratch-{}", uuid::Uuid::new_v4());
        artifacts::write_bytes(&root, &format!("{name}/model.qnt"), &source, MODEL_LIMIT)?;
        let scratch = Scratch(root.join(name));
        let dir = &scratch.0;
        let quint = tools.join("node_modules/@informalsystems/quint/dist/src/cli.js");
        let typecheck = run(
            dir,
            &quint,
            &["typecheck", "model.qnt"],
            &token,
            "typecheck",
        )?;
        let mut args = vec![
            "test",
            "model.qnt",
            "--backend",
            "typescript",
            "--seed",
            "57",
        ];
        if render {
            args.extend(["--out-itf", "test_{test}_{seq}.itf.json"]);
        }
        let result = run(dir, &quint, &args, &token, "test")?;
        let tests = result
            .lines()
            .find_map(|line| {
                let (count, _) = line.trim().split_once(" passing")?;
                count.parse::<usize>().ok().filter(|count| *count > 0)
            })
            .ok_or_else(|| error("Quint must report at least one passing test"))?;
        let mut log = format!(
            "quint typecheck model.qnt: exit 0\n{typecheck}\nquint {}: exit 0\n{result}",
            args.join(" ")
        );
        let png = if render {
            artifacts::write(dir, "render.cjs", RENDERER)?;
            let path = tools
                .to_str()
                .ok_or_else(|| error("Quint tool path is not UTF-8"))?;
            let rendered = run(dir, &dir.join("render.cjs"), &[".", path], &token, "render")?;
            let _ = write!(log, "\nQuint diagram: {rendered}");
            let bytes = artifacts::read_bytes(dir, "diagram.png", MAX_PNG)?;
            super::ensure_png(&bytes)?;
            Some(bytes)
        } else {
            None
        };
        Ok(Checked { png, tests, log })
    })
    .await
    .map_err(|join| error(format!("Quint check task failed: {join}")))?
}
