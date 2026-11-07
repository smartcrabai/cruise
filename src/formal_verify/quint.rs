use super::{BlockReport, SpecBlock, StageReport, Verdict, process::run_bounded, tools};
use crate::cancellation::CancellationToken;
use std::{path::Path, process::Command, time::Duration};
#[derive(Debug)]
pub struct MainModule {
    pub name: String,
    pub invariants: Vec<String>,
    pub temporals: Vec<String>,
}
/// Finds the main module and its verification targets in a Quint AST.
///
/// # Errors
/// Returns an error for invalid JSON, malformed modules, or multiple main modules.
pub fn main_module(ast: &str) -> Result<Option<MainModule>, String> {
    let value: serde_json::Value = serde_json::from_str(ast).map_err(|e| e.to_string())?;
    let modules = value
        .get("modules")
        .and_then(|x| x.as_array())
        .ok_or("invalid AST modules")?;
    let mut found = Vec::new();
    for m in modules {
        let Some(decls) = m.get("declarations").and_then(|v| v.as_array()) else {
            continue;
        };
        let has = |name: &str| {
            decls
                .iter()
                .any(|d| d["kind"] == "def" && d["qualifier"] == "action" && d["name"] == name)
        };
        if has("init") && has("step") {
            let name = m["name"].as_str().ok_or("module missing name")?.to_owned();
            let mut item = MainModule {
                name,
                invariants: vec![],
                temporals: vec![],
            };
            for d in decls {
                if d["kind"] != "def" {
                    continue;
                }
                let Some(name) = d["name"].as_str() else {
                    continue;
                };
                if d["qualifier"] == "val" && name.starts_with("inv") {
                    item.invariants.push(name.into());
                }
                if d["qualifier"] == "temporal" && name.starts_with("prop") {
                    item.temporals.push(name.into());
                }
            }
            found.push(item);
        }
    }
    if found.len() > 1 {
        Err("ambiguous main module".into())
    } else {
        Ok(found.pop())
    }
}
#[must_use]
pub fn quint_verdict(exit: Option<i32>, output: &str, timed_out: bool) -> Verdict {
    if timed_out {
        Verdict::Error
    } else if exit == Some(0) {
        Verdict::Passed
    } else if exit.is_some_and(|code| code != 0) && output.contains("[violation]") {
        Verdict::Failed
    } else {
        Verdict::Error
    }
}
pub fn verify(
    block: &SpecBlock,
    dir: &Path,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
    warnings: &mut Vec<String>,
) -> BlockReport {
    let mut report = BlockReport {
        language: block.language,
        number: block.number,
        stages: vec![],
        artifact: dir.into(),
    };
    let logs = dir.join("logs");
    match tools::check_quint(&logs, cancel) {
        Ok(Some(w)) => warnings.push(w),
        Ok(None) => {}
        Err(e) => {
            report.error("tool", e);
            return report;
        }
    }
    if let Err(e) = std::fs::write(dir.join("model.qnt"), &block.source) {
        report.error("write", e.to_string());
        return report;
    }
    let run = |args: Vec<String>, stage: &str, limit: Duration| -> StageReport {
        let mut c = Command::new("quint");
        c.current_dir(dir).args(args);
        let out = run_bounded(c, limit, cancel, &logs, stage);
        let log = out.output();
        StageReport {
            name: stage.into(),
            verdict: quint_verdict(
                out.exit,
                if out.has_violation() {
                    "[violation]"
                } else {
                    &log
                },
                out.timed_out || out.cancelled,
            ),
            message: if out.cancelled {
                "cancelled".into()
            } else {
                out.message
            },
            timed_out: out.timed_out,
            log,
        }
    };
    let parse = run(
        vec![
            "parse".into(),
            "--out".into(),
            dir.join("ast.json").to_string_lossy().into_owned(),
            "model.qnt".into(),
        ],
        "parse",
        Duration::from_secs(60),
    );
    let ok = parse.verdict == Verdict::Passed;
    report.stages.push(parse);
    if !ok {
        return report;
    }
    let module = match std::fs::read_to_string(dir.join("ast.json"))
        .map_err(|e| e.to_string())
        .and_then(|s| main_module(&s))
    {
        Ok(Some(m)) => m,
        Ok(None) => {
            report.error("main module", "No main module with action init and step");
            return report;
        }
        Err(e) => {
            report.error("main module", e);
            return report;
        }
    };
    if module.invariants.is_empty() && module.temporals.is_empty() {
        report.error("targets", "No verification targets found");
        return report;
    }
    let stage = run(
        vec!["typecheck".into(), "model.qnt".into()],
        "typecheck",
        Duration::from_secs(60),
    );
    let ok = stage.verdict == Verdict::Passed;
    report.stages.push(stage);
    if !ok {
        return report;
    }
    verify_targets(report, &module, dir, timeout, &run)
}

fn verify_targets(
    mut report: BlockReport,
    module: &MainModule,
    dir: &Path,
    timeout: Duration,
    run: &impl Fn(Vec<String>, &str, Duration) -> StageReport,
) -> BlockReport {
    let common = || {
        vec![
            "--main".into(),
            module.name.clone(),
            "--init".into(),
            "init".into(),
            "--step".into(),
            "step".into(),
        ]
    };
    if !module.invariants.is_empty() {
        let mut args = vec!["run".into(), "--backend".into(), "typescript".into()];
        args.extend(common());
        args.push("--invariants".into());
        args.extend(module.invariants.iter().cloned());
        args.extend([
            "--max-samples".into(),
            "1".into(),
            "--max-steps".into(),
            "20".into(),
            "--seed".into(),
            "1".into(),
            "model.qnt".into(),
        ]);
        let stage = run(args, "run", Duration::from_secs(60));
        let ok = stage.verdict == Verdict::Passed;
        report.stages.push(stage);
        if !ok {
            return report;
        }
        let port = match std::net::TcpListener::bind("127.0.0.1:0").and_then(|s| s.local_addr()) {
            Ok(a) => a.port(),
            Err(e) => {
                report.error("port", e.to_string());
                return report;
            }
        };
        let mut args = vec!["verify".into()];
        args.extend(common());
        args.push("--invariants".into());
        args.extend(module.invariants.iter().cloned());
        args.extend([
            "--max-steps".into(),
            "20".into(),
            "--backend".into(),
            "apalache".into(),
            "--apalache-version".into(),
            "0.62.1".into(),
            "--server-endpoint".into(),
            format!("127.0.0.1:{port}"),
            "--out-itf".into(),
            dir.join("counterexample.itf.json")
                .to_string_lossy()
                .into_owned(),
            "model.qnt".into(),
        ]);
        let stage = run(args, "verify-invariants", timeout);
        let ok = stage.verdict == Verdict::Passed;
        report.stages.push(stage);
        if !ok {
            return report;
        }
    }
    if !module.temporals.is_empty() {
        let mut args = vec!["verify".into()];
        args.extend(common());
        args.extend([
            "--temporal".into(),
            module.temporals.join(","),
            "--max-steps".into(),
            "20".into(),
            "--backend".into(),
            "tlc".into(),
            "model.qnt".into(),
        ]);
        report.stages.push(run(args, "verify-temporal", timeout));
    }
    report
}
