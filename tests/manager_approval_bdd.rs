//! Production-connected approval boundary scenarios, using isolated Git repositories.
use cruise::manager::{self, Check, Criterion, GoalStatus, Proposal, Task};
use cucumber::{World as _, given, then, when};
use std::{path::Path, process::Command};

#[derive(Debug, Default, cucumber::World)]
struct World {
    root: Option<tempfile::TempDir>,
    first: Option<Proposal>,
    current: Option<Proposal>,
    goal_ids: Vec<String>,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl World {
    fn repo(&self) -> &Path {
        self.root.as_ref().unwrap().path()
    }
    fn input(&self) -> Proposal {
        Proposal {
            id: "provider-id".into(),
            revision: 42,
            objective: "Deliver goal".into(),
            target_branch: "main".into(),
            target_sha: git(self.repo(), &["rev-parse", "HEAD"]),
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
            tasks: vec![Task {
                id: "task".into(),
                instructions: "Implement".into(),
                dependencies: vec![],
            }],
            workflow: "builtin:tdd".into(),
            contract: None,
        }
    }
}

#[given("an isolated repository with a manager proposal")]
fn repository(world: &mut World) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path();
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.name", "Contract Test"]);
    git(repo, &["config", "user.email", "contract@example.test"]);
    std::fs::write(repo.join("file"), "base\n").unwrap();
    git(repo, &["add", "file"]);
    git(repo, &["commit", "-qm", "base"]);
    let sha = git(repo, &["rev-parse", "HEAD"]);
    git(
        repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    git(repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    world.root = Some(root);
    let p = manager::save_proposal(world.repo(), world.input()).unwrap();
    let snapshot = manager::snapshot(world.repo()).unwrap();
    assert_eq!(snapshot.proposal.unwrap().id, p.id);
    assert!(snapshot.goals.is_empty());
    world.first = Some(p.clone());
    world.current = Some(p);
}

#[when("the proposal is replaced by a newer preview")]
fn replace(world: &mut World) {
    let current = manager::save_proposal(world.repo(), world.input()).unwrap();
    let first = world.first.as_ref().unwrap();
    assert_ne!(first.id, current.id);
    assert_eq!(first.revision + 1, current.revision);
    world.current = Some(current);
}

#[when("approvals are attempted with the old identity and an old revision")]
fn stale(world: &mut World) {
    let first = world.first.as_ref().unwrap();
    let current = world.current.as_ref().unwrap();
    assert!(manager::approve(world.repo(), &first.id, first.revision).is_err());
    assert!(manager::approve(world.repo(), &current.id, first.revision).is_err());
}

#[then("the newer preview remains and no goal or child attempt exists")]
fn no_goal(world: &mut World) {
    let snapshot = manager::snapshot(world.repo()).unwrap();
    let p = snapshot.proposal.unwrap();
    let expected = world.current.as_ref().unwrap();
    assert_eq!((p.id, p.revision), (expected.id.clone(), expected.revision));
    assert!(snapshot.goals.is_empty());
}

#[when("the current proposal is approved twice")]
fn approve(world: &mut World) {
    let p = world.current.as_ref().unwrap();
    for _ in 0..2 {
        world.goal_ids.push(
            manager::approve(world.root.as_ref().unwrap().path(), &p.id, p.revision)
                .unwrap()
                .id,
        );
    }
}

#[then("one active goal exists for that exact proposal without child attempts")]
fn one_goal(world: &mut World) {
    let snapshot = manager::snapshot(world.repo()).unwrap();
    assert!(snapshot.proposal.is_none());
    assert_eq!(snapshot.goals.len(), 1);
    let goal = &snapshot.goals[0];
    let p = world.current.as_ref().unwrap();
    assert_eq!(world.goal_ids, [goal.id.clone(), goal.id.clone()]);
    assert_eq!(
        (&goal.proposal.id, goal.proposal.revision),
        (&p.id, p.revision)
    );
    assert_eq!(goal.status, GoalStatus::Active);
    assert!(goal.execution.attempts.is_empty());
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Set once before Cucumber starts, never concurrently with other threads.
    let data = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", data.path());
        std::env::set_var("XDG_DATA_HOME", data.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", data.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", data.path().join("state"));
    }
    World::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/manager_approval.feature"
        ))
        .await;
}
