#![cfg(unix)]

//! UI E2E tooling guard through the real CLI, a guarded prompt, and a fake agent.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

/// The built-in `e2e-baseline` / `e2e` step pair with a scripted agent.
const WORKFLOW: &str = r#"command: [sh, "AGENT"]
max_retries: 2
steps:
  e2e-baseline:
    command: '{cruise.bin} internal e2e-baseline e2e'
    env:
      CRUISE_E2E_PLAN: '{plan}'
    accepted_exit_codes: [0]
  e2e:
    prompt: run UI E2E
    output_file: e2e/report.md
    env:
      CRUISE_E2E_PLAN: '{plan}'
    quality_gates:
      - command: '{cruise.bin} internal e2e-guard e2e --report e2e/report.md'
        stop_exit_codes: [75]
  later:
    command: 'touch later-ran'
"#;

impl Fixture {
    fn new(agent: &str) -> TestResult<Self> {
        let root = tempfile::tempdir()?;
        let repo = root.path().join("repo");
        let home = root.path().join("home");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(&home)?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        std::fs::write(repo.join("package.json"), "{\"dependencies\":{}}\n")?;
        git(&repo, &["add", "package.json"])?;
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        )?;
        let agent_path = home.join("agent.sh");
        std::fs::write(
            &agent_path,
            format!(
                "echo attempt >> \"$HOME/attempts\"\nn=$(wc -l < \"$HOME/attempts\" | tr -d ' ')\ncat > \"$HOME/prompt-$n\"\n{agent}"
            ),
        )?;
        let config = root.path().join("workflow.yaml");
        std::fs::write(
            &config,
            WORKFLOW.replace("AGENT", &agent_path.display().to_string()),
        )?;
        Ok(Self {
            root,
            repo,
            home,
            config,
        })
    }

    fn exec(&self) -> TestResult<Output> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        Ok(cmd
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .args(["exec", "--config"])
            .arg(&self.config)
            .arg("task")
            .output()?)
    }

    fn attempts(&self) -> usize {
        std::fs::read_to_string(self.home.join("attempts"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

fn git(repo: &Path, args: &[&str]) -> TestResult {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_COUNT", "0")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn dependency_change_is_rejected_then_blocked_report_stops_the_session() -> TestResult {
    // Attempt 1 downgrades a dependency and adds Playwright; attempt 2 restores
    // the manifest, removes the framework, and reports the missing simulator.
    let f = Fixture::new(
        "if [ \"$n\" = 1 ]; then\n  printf '{\"dependencies\":{\"react-native\":\"0.86.3\"}}\\n' > package.json\n  echo 'export default {};' > playwright.config.ts\n  printf 'E2E_STATUS: passed\\n'\nelse\n  git checkout -q -- package.json\n  rm -f playwright.config.ts\n  printf '## Blockers\\n- iOS: Maestro is not installed; install Maestro and boot a simulator.\\n\\nE2E_STATUS: blocked\\n'\nfi\n",
    )?;
    let out = f.exec()?;
    let output = text(&out);
    assert!(!out.status.success(), "{output}");
    assert_eq!(f.attempts(), 2, "{output}");
    assert!(!f.repo.join("later-ran").exists(), "{output}");
    assert!(
        output.contains("quality gate stopped step 'e2e'")
            && output.contains("install Maestro and boot a simulator"),
        "{output}"
    );

    let retry_prompt = std::fs::read_to_string(f.home.join("prompt-2"))?;
    assert!(
        retry_prompt.contains("M package.json")
            && retry_prompt.contains("A playwright.config.ts (new E2E framework: Playwright)"),
        "{retry_prompt}"
    );
    Ok(())
}

#[test]
fn passed_report_with_only_test_changes_continues() -> TestResult {
    let f = Fixture::new(
        "mkdir -p e2e && echo 'flow' > e2e/reply.flow\nprintf 'Targets: web\\nE2E_STATUS: passed\\n'\n",
    )?;
    let out = f.exec()?;
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.attempts(), 1);
    assert!(f.repo.join("later-ran").exists());
    Ok(())
}
