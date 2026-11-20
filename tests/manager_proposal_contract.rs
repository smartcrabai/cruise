use cruise::manager::{self, Check, Criterion, Proposal, Task};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn proposal(sha: &str) -> Proposal {
    Proposal {
        id: "provider-id".into(),
        revision: 42,
        objective: "Deliver the goal".into(),
        target_branch: "main".into(),
        target_sha: sha.into(),
        exclusions: vec![],
        criteria: vec![Criterion {
            id: "c".into(),
            description: "Works".into(),
        }],
        checks: vec![Check {
            id: "check".into(),
            criterion_ids: vec!["c".into()],
            argv: vec!["true".into()],
        }],
        tasks: vec![
            Task {
                id: "a".into(),
                instructions: "Implement".into(),
                dependencies: vec![],
            },
            Task {
                id: "b".into(),
                instructions: "Test".into(),
                dependencies: vec!["a".into()],
            },
        ],
        workflow: "builtin:tdd".into(),
        contract: None,
    }
}

#[test]
fn invalid_dependency_cycles_and_missing_dependencies_are_rejected() {
    let mut p = proposal("sha");
    assert!(p.validate().is_ok());
    p.tasks[0].dependencies = vec!["a".into()];
    assert!(p.validate().is_err());
    p.tasks[0].dependencies = vec!["b".into()];
    assert!(p.validate().is_err());
    p.tasks[0].dependencies = vec!["missing".into()];
    assert!(p.validate().is_err());
}

// Each invocation gets an isolated XDG directory without changing the test runner's environment.
#[test]
fn durable_proposal_and_approval_contract() {
    if std::env::var_os("MANAGER_CONTRACT_CHILD").is_none() {
        let root = TempDir::new().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_proposal_and_approval_contract",
                "--nocapture",
            ])
            .env("MANAGER_CONTRACT_CHILD", root.path())
            .env("HOME", root.path())
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_STATE_HOME", root.path().join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("MANAGER_CONTRACT_CHILD").unwrap());
    let repo = root.join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Contract Test"]);
    git(&repo, &["config", "user.email", "contract@example.test"]);
    fs::write(repo.join("file"), "base\n").unwrap();
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);

    let first = manager::save_proposal(&repo, proposal(&sha)).unwrap();
    let loaded = manager::snapshot(&repo).unwrap();
    assert_eq!(loaded.proposal.unwrap().id, first.id);
    assert!(loaded.goals.is_empty());
    let second = manager::save_proposal(&repo, proposal(&sha)).unwrap();
    assert_eq!(second.revision, first.revision + 1);
    assert_ne!(second.id, first.id);
    assert!(manager::approve(&repo, &first.id, first.revision).is_err());
    assert!(manager::approve(&repo, &second.id, first.revision).is_err());
    let after_stale = manager::snapshot(&repo).unwrap();
    assert_eq!(after_stale.proposal.unwrap().id, second.id);
    assert!(after_stale.goals.is_empty());
    let goal = manager::approve(&repo, &second.id, second.revision).unwrap();
    let persisted = manager::snapshot(&repo).unwrap();
    assert!(persisted.proposal.is_none());
    assert_eq!(persisted.goals.len(), 1);
    assert_eq!(persisted.goals[0].id, goal.id);
    assert_eq!(persisted.goals[0].proposal.id, second.id);
    assert_eq!(
        manager::approve(&repo, &second.id, second.revision)
            .unwrap()
            .id,
        goal.id
    );
    assert_eq!(manager::snapshot(&repo).unwrap().goals.len(), 1);
}
