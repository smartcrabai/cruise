use cruise::manager::{self, Check, Criterion, Proposal, Task};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn isolated<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("JCODE_HOME", home.join("jcode"))
        .env("LANG", "en_US.UTF-8")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("CRUISE_COMMAND", r#"["/bin/sleep","60"]"#)
        .stdin(Stdio::null())
}

pub fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{output:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap_or_else(|err| panic!("decode command stdout: {err}"))
}

fn git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    success(
        isolated(&mut command, repo)
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("run fixture Git command: {err}")),
    )
    .trim()
    .into()
}

pub struct Fixture {
    _root: TempDir,
    pub home: PathBuf,
    pub repo: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("mcli-")
            .tempdir()
            .unwrap_or_else(|err| panic!("create fixture root: {err}"));
        let home = root.path().join("home");
        let repo = root.path().join("repo");
        std::fs::create_dir(&home).unwrap_or_else(|err| panic!("create fixture home: {err}"));
        std::fs::create_dir(&repo).unwrap_or_else(|err| panic!("create fixture repo: {err}"));
        std::fs::create_dir_all(home.join("config/cruise"))
            .unwrap_or_else(|err| panic!("create fixture config directory: {err}"));
        std::fs::write(
            home.join("config/cruise/default.yaml"),
            "check_base_ci: false\n",
        )
        .unwrap_or_else(|err| panic!("write fixture defaults: {err}"));
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "-qm",
                "base",
                "--allow-empty",
            ],
        );
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/project.git",
            ],
        );
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let fixture = Self {
            _root: root,
            home,
            repo,
        };
        success(
            fixture
                .command()
                .args(["daemon", "start", "--port", "0"])
                .output()
                .unwrap_or_else(|err| panic!("start fixture daemon: {err}")),
        );
        fixture
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        isolated(&mut command, &self.home).current_dir(&self.repo);
        command
    }

    pub fn manager(&self, args: &[&str]) -> Output {
        self.command()
            .arg("manager")
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("run Manager command: {err}"))
    }

    pub async fn api(&self, action: Option<Value>) -> Value {
        let instance: Value = serde_json::from_slice(
            &std::fs::read(self.home.join("state/cruise/daemon/instance.json"))
                .unwrap_or_else(|err| panic!("read daemon instance: {err}")),
        )
        .unwrap_or_else(|err| panic!("decode daemon instance: {err}"));
        let origin = instance["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("find daemon origin"));
        let token = instance["token"]
            .as_str()
            .unwrap_or_else(|| panic!("find daemon token"));
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|err| panic!("build fixture HTTP client: {err}"));
        let request = if let Some(action) = action {
            client
                .post(format!("{origin}api/manager/action"))
                .json(&serde_json::json!({"cwd":self.repo, "action":action}))
        } else {
            let mut url = reqwest::Url::parse(&format!("{origin}api/manager/snapshot"))
                .unwrap_or_else(|err| panic!("parse snapshot URL: {err}"));
            url.query_pairs_mut().append_pair(
                "cwd",
                self.repo
                    .to_str()
                    .unwrap_or_else(|| panic!("encode repository path")),
            );
            client.get(url)
        };
        let response = request
            .bearer_auth(token)
            .send()
            .await
            .unwrap_or_else(|err| panic!("send Manager API request: {err}"));
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|err| panic!("read Manager API response: {err}"));
        assert!(status.is_success(), "manager fixture API: {status} {body}");
        serde_json::from_str(&body)
            .unwrap_or_else(|err| panic!("decode Manager API response: {err}"))
    }

    pub fn snapshot(&self) -> Value {
        serde_json::from_str(&success(self.manager(&["status", "--json"])))
            .unwrap_or_else(|err| panic!("decode Manager CLI snapshot: {err}"))
    }

    pub fn seed_option_proposal(&self) -> Proposal {
        let marker = self.home.join("option-answer");
        let marker = marker
            .to_str()
            .unwrap_or_else(|| panic!("encode marker path"))
            .replace('{', "{{")
            .replace('}', "}}")
            .replace('\'', "'\\''");
        let capture = format!("printf '%s' \"{{prev.input}}\" > '{marker}'");
        self.seed_prompt_workflow(serde_json::json!({
            "command":["/bin/cat"], "create_pr":false, "cleanup_after_pr":false,
            "steps":{"a_choose":{"option":[{"text-input":"Provide a value", "next":"b_capture"}]}, "b_capture":{"command":capture}}
        }))
    }

    pub fn seed_prompt_workflow(&self, workflow: Value) -> Proposal {
        let proposal = self.seed_proposal();
        // The daemon creates the owned child and its live prompt from this record.
        let directory = self.home.join("data/cruise/manager");
        let mut records = std::fs::read_dir(directory)
            .unwrap_or_else(|err| panic!("list Manager records: {err}"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|err| panic!("read Manager record entry: {err}"))
                    .path()
            });
        let record = records
            .next()
            .unwrap_or_else(|| panic!("find Manager record"));
        assert!(records.next().is_none());
        let mut snapshot: Value = serde_json::from_slice(
            &std::fs::read(&record).unwrap_or_else(|err| panic!("read Manager record: {err}")),
        )
        .unwrap_or_else(|err| panic!("decode Manager record: {err}"));
        let workflow: cruise::config::WorkflowConfig = serde_json::from_value(workflow)
            .unwrap_or_else(|err| panic!("decode fixture workflow: {err}"));
        snapshot["proposal"]["contract"]["workflow"] = serde_json::to_value(workflow)
            .unwrap_or_else(|err| panic!("encode fixture workflow: {err}"));
        std::fs::write(
            record,
            serde_json::to_vec(&snapshot)
                .unwrap_or_else(|err| panic!("encode Manager snapshot: {err}")),
        )
        .unwrap_or_else(|err| panic!("write Manager snapshot: {err}"));
        proposal
    }

    pub fn seed_proposal(&self) -> Proposal {
        // Scope environment changes to the fixture subprocess, not the test runner.
        let path = self.home.join("proposal.json");
        let mut command = Command::new(
            std::env::current_exe()
                .unwrap_or_else(|err| panic!("locate fixture executable: {err}")),
        );
        success(
            isolated(&mut command, &self.home)
                .env("MANAGER_CLI_SEED", &path)
                .current_dir(&self.repo)
                .args(["--ignored", "--exact", "support::save_proposal_fixture"])
                .output()
                .unwrap_or_else(|err| panic!("run proposal fixture subprocess: {err}")),
        );
        serde_json::from_slice(
            &std::fs::read(path).unwrap_or_else(|err| panic!("read seeded proposal: {err}")),
        )
        .unwrap_or_else(|err| panic!("decode seeded proposal: {err}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop", "--force"]).output();
    }
}

#[test]
#[ignore = "subprocess-only proposal fixture"]
fn save_proposal_fixture() {
    let Some(path) = std::env::var_os("MANAGER_CLI_SEED") else {
        return;
    };
    let repo = std::env::current_dir()
        .unwrap_or_else(|err| panic!("read fixture working directory: {err}"));
    let proposal = manager::save_proposal(
        &repo,
        Proposal {
            id: String::new(),
            revision: 0,
            objective: "Deliver the reviewed CLI goal".into(),
            target_branch: "main".into(),
            target_sha: git(&repo, &["rev-parse", "HEAD"]),
            exclusions: vec!["Do not change the public API".into()],
            criteria: vec![Criterion {
                id: "success".into(),
                description: "The CLI works".into(),
            }],
            checks: vec![Check {
                id: "review-check".into(),
                criterion_ids: vec!["success".into()],
                argv: vec!["true".into()],
            }],
            tasks: vec![Task {
                id: "task".into(),
                instructions: "Implement the approved work".into(),
                dependencies: vec![],
            }],
            workflow: "builtin:tdd".into(),
            contract: None,
        },
    )
    .unwrap_or_else(|err| panic!("save fixture proposal: {err}"));
    std::fs::write(
        path,
        serde_json::to_vec(&proposal).unwrap_or_else(|err| panic!("encode seeded proposal: {err}")),
    )
    .unwrap_or_else(|err| panic!("write seeded proposal: {err}"));
}
