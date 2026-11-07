use super::{BlockReport, SpecBlock, StageReport, Verdict, process::run_bounded, tools};
use crate::cancellation::CancellationToken;
use std::{path::Path, process::Command, time::Duration};

#[derive(Clone, Debug)]
pub struct AlloyCommand {
    pub kind: String,
    pub name: String,
}

/// Parse the pinned Alloy CLI's `commands` output, not the model source.
///
/// # Errors
/// Returns an error if the listing is empty or contains an invalid command.
pub fn alloy_commands(listing: &str) -> Result<Vec<AlloyCommand>, String> {
    let mut commands = Vec::new();
    for line in listing.lines() {
        let (index, description) = line
            .split_once('.')
            .ok_or("Invalid Alloy command listing")?;
        if index.trim().parse::<usize>().ok() != Some(commands.len()) {
            return Err("Invalid Alloy command index".into());
        }
        let mut words = description.split_whitespace();
        let kind = match words.next() {
            Some("Run") => "run",
            Some("Check") => "check",
            _ => return Err("Invalid Alloy command kind".into()),
        };
        let name = words.next().ok_or("Missing Alloy command label")?;
        commands.push(AlloyCommand {
            kind: kind.into(),
            name: name.into(),
        });
    }
    if commands.is_empty() {
        Err("No Alloy commands found".into())
    } else {
        Ok(commands)
    }
}

#[must_use]
pub fn receipt_verdict(kind: &str, name: &str, json: &str) -> Verdict {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Verdict::Error;
    };
    let Some(command) = value.get("commands").and_then(|x| x.get(name)) else {
        return Verdict::Error;
    };
    if command.get("type").and_then(|x| x.as_str()) != Some(kind) {
        return Verdict::Error;
    }
    let sat = match command.get("solution") {
        None => false,
        Some(s) => {
            let Some(a) = s.as_array() else {
                return Verdict::Error;
            };
            if a.is_empty()
                || a.iter().any(|x| {
                    x.get("instances")
                        .and_then(|v| v.as_array())
                        .is_none_or(Vec::is_empty)
                })
            {
                return Verdict::Error;
            }
            true
        }
    };
    match (kind, sat) {
        ("run", true) | ("check", false) => Verdict::Passed,
        ("run", false) | ("check", true) => Verdict::Failed,
        _ => Verdict::Error,
    }
}

pub fn verify(
    block: &SpecBlock,
    dir: &Path,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> BlockReport {
    let mut report = BlockReport {
        language: block.language,
        number: block.number,
        stages: vec![],
        artifact: dir.into(),
    };
    let logs = dir.join("logs");
    if let Err(e) = tools::check_java(&logs, cancel) {
        report.error("tool", e);
        return report;
    }
    let jar = match tools::alloy_jar(timeout, cancel) {
        Ok(p) => p,
        Err(e) => {
            report.error("Alloy JAR", e);
            return report;
        }
    };
    if let Err(e) = std::fs::write(dir.join("model.als"), &block.source) {
        report.error("write", e.to_string());
        return report;
    }
    let mut discovery = Command::new("java");
    discovery
        .current_dir(dir)
        .args(["-Xmx4g", "-Xss64m", "-jar"])
        .arg(&jar)
        .args(["commands", "model.als"]);
    let outcome = run_bounded(discovery, timeout, cancel, &logs, "commands");
    if outcome.timed_out || outcome.cancelled || outcome.exit != Some(0) {
        report.error(
            "commands",
            format!(
                "Alloy command discovery failed: {} {}",
                outcome.message,
                outcome.output()
            ),
        );
        return report;
    }
    let commands = match std::fs::read_to_string(&outcome.stdout)
        .map_err(|e| e.to_string())
        .and_then(|listing| alloy_commands(&listing))
    {
        Ok(commands) => commands,
        Err(e) => {
            report.error("commands", e);
            return report;
        }
    };
    for (index, item) in commands.into_iter().enumerate() {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            break;
        }
        let selector = index.to_string();
        let output_dir = dir.join(format!("command-{index}"));
        let mut c = Command::new("java");
        c.current_dir(dir)
            .args(["-Xmx4g", "-Xss64m", "-jar"])
            .arg(&jar)
            .args(["exec", "--command", &selector, "--type", "json", "--output"])
            .arg(&output_dir)
            .arg("model.als");
        let outcome = run_bounded(c, timeout, cancel, &logs, &format!("command-{index}"));
        let verdict = if outcome.timed_out || outcome.cancelled || outcome.exit != Some(0) {
            Verdict::Error
        } else {
            std::fs::read_to_string(output_dir.join("receipt.json")).map_or(Verdict::Error, |s| {
                receipt_verdict(&item.kind, &item.name, &s)
            })
        };
        let log = outcome.output();
        report.stages.push(StageReport {
            name: format!("{} {}", item.kind, item.name),
            verdict,
            message: outcome.message,
            timed_out: outcome.timed_out,
            log,
        });
    }
    report
}
