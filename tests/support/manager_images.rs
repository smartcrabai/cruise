use super::{
    ManagerCli, Stop, cli_fixture, cli_stdout, daemon_client, setup_repo, shell_quote, snapshot,
    start, wait_for,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgaPgPAAIDAYAkYfWXAAAAAElFTkSuQmCC";

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(result, "{byte:02x}").unwrap_or_else(|error| panic!("hash fixture: {error}"));
    }
    result
}

fn put(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::write(path, bytes)
        .unwrap_or_else(|error| panic!("write fixture {}: {error}", path.display()));
}

fn bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()))
}

fn string(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("missing value: {value}"))
}

fn manager_root(home: &Path, repo: &Path) -> (PathBuf, String) {
    let common = repo
        .join(".git")
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonical fixture: {error}"));
    (
        home.join("data/cruise/manager"),
        digest(common.to_string_lossy().as_bytes()),
    )
}

pub(super) fn capture_image(release_path: &Path, serialized: &str) {
    let request: Value =
        serde_json::from_str(serialized).unwrap_or_else(|error| panic!("decode request: {error}"));
    let image = request
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
        .filter_map(Value::as_str)
        .find_map(|value| value.split_once("Attached images (use the Read tool to view):\n- "))
        .and_then(|(_, paths)| paths.lines().next())
        .unwrap_or_else(|| panic!("SDK missing image input"));
    let proof = release_path.with_file_name("provider-image.png");
    assert!(!proof.exists(), "unexpected replay of image SDK request");
    put(&proof, bytes(Path::new(image)));
}

pub(super) fn fake_proposal(fixture: &str, valid: &str) -> Option<String> {
    let (_, value) = fixture.split_once("image-proposal:")?;
    let (task, image) = value.split_once(':')?;
    let mut proposal: Value =
        serde_json::from_str(valid).unwrap_or_else(|error| panic!("decode proposal: {error}"));
    let mut second = proposal["tasks"][0].clone();
    second["id"] = json!("other");
    second["dependencies"] = json!(["task"]);
    proposal["tasks"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("missing tasks"))
        .push(second);
    proposal["task_images"][task] = json!([image]);
    Some(proposal.to_string())
}

struct Run<'a> {
    home: &'a Path,
    repo: &'a Path,
    origin: String,
    token: String,
    client: reqwest::Client,
}

impl<'a> Run<'a> {
    fn start(home: &'a Path, repo: &'a Path) -> Self {
        let (origin, token) = start(home, true);
        Self {
            home,
            repo,
            origin,
            token,
            client: daemon_client(),
        }
    }

    async fn manager(&self, args: &[&str]) -> std::process::Output {
        ManagerCli::spawn(self.home, self.repo, args, None)
            .finish()
            .await
    }

    async fn snapshot(&self) -> Value {
        snapshot(&self.client, &self.origin, &self.token, self.repo).await
    }

    async fn reject(&self, args: &[&str], message: &str) {
        let output = self.manager(args).await;
        assert!(!output.status.success(), "unexpected success: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stderr.contains(message) || stdout.contains(message),
            "{stdout}{stderr}"
        );
    }
}

fn use_image_read_step(record: &Path, proof: &Path) {
    let mut saved: Value = serde_json::from_slice(&bytes(record))
        .unwrap_or_else(|error| panic!("decode fixture: {error}"));
    let command = format!(
        "path=$(sed -n 's/^- \\(.*\\.png\\)$/\\1/p' '{{plan}}'); if [ -n \"$path\" ]; then cp \"$path\" {} && echo attached > task-status; else echo no-image > other-status; fi",
        shell_quote(proof),
    );
    saved["proposal"]["contract"]["workflow"] = json!({
        "command":["sh"],"create_pr":false,"steps":{"read-image":{"command":command}}
    });
    put(record, saved.to_string());
}

#[tokio::test]
async fn manager_images_survive_restart_and_only_reach_approved_task() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let config = home.join("config/cruise");
    fs::create_dir_all(&config).unwrap_or_else(|error| panic!("create fixture config: {error}"));
    put(&config.join("default.yaml"), "check_base_ci: false\n");
    let png = STANDARD
        .decode(PNG)
        .unwrap_or_else(|error| panic!("decode PNG fixture: {error}"));
    let file = home.join("sample.png");
    let released = home.join("release-jcode");
    let provider = home.join("provider-image.png");
    put(&file, &png);
    put(&released, "capture-image");
    let run = Run::start(home, &repo);
    let stop = Stop(home);
    cli_stdout(
        &run.manager(&[
            "say",
            "Screenshot",
            "--image",
            file.to_str().unwrap_or_default(),
        ])
        .await,
    );
    assert_eq!(bytes(&provider), png);
    fs::remove_file(&file).unwrap_or_else(|error| panic!("remove original fixture: {error}"));
    let sent = run.snapshot().await;
    let turn = &sent["transcript"][0];
    let image = &turn["images"][0];
    let id = string(&image["id"]);
    let (dir, repo_key) = manager_root(home, &repo);
    let source = dir.join("images").join(&repo_key).join(format!("{id}.png"));
    assert_eq!(image["sha256"], digest(&png));
    assert_eq!(bytes(&source), png);
    drop(stop);

    let run = Run::start(home, &repo);
    let _stop = Stop(home);
    assert_eq!(run.snapshot().await["transcript"][0], *turn);
    run.reject(
        &["say", "--retry-image-turn", string(&turn["id"])],
        "not eligible to retry",
    )
    .await;
    let foreign_path = home.join("foreign-home");
    fs::create_dir(&foreign_path).unwrap_or_else(|error| panic!("foreign fixture home: {error}"));
    let foreign_repo = setup_repo(&foreign_path);
    let foreign = Run {
        home,
        repo: &foreign_repo,
        origin: run.origin.clone(),
        token: run.token.clone(),
        client: run.client.clone(),
    };
    put(&released, format!("image-proposal:task:{id}"));
    foreign
        .reject(&["propose"], "not from a completed turn in this repository")
        .await;
    put(&released, format!("image-proposal:unknown:{id}"));
    run.reject(&["propose"], "Invalid Manager task image assignment")
        .await;
    put(&released, format!("image-proposal:task:{id}"));
    cli_stdout(&run.manager(&["propose"]).await);
    let proposal = run.snapshot().await["proposal"].clone();
    assert_eq!(proposal["contract"]["task_images"], json!({"task":[image]}));
    let record = dir.join(format!("{repo_key}.json"));
    approve_and_check_copy(&run, &record, &source, &proposal, image, &png).await;
    assert_eq!(bytes(&provider), png);
}

async fn approve_and_check_copy(
    run: &Run<'_>,
    record: &Path,
    source: &Path,
    proposal: &Value,
    image: &Value,
    png: &[u8],
) {
    let home = run.home;
    let id = string(&image["id"]);
    let proof = home.join("child-image.png");
    use_image_read_step(record, &proof);
    let rev = proposal["revision"].to_string();
    assert!(
        !run.manager(&["approve", "--yes", "--revision", "0"])
            .await
            .status
            .success()
    );
    let mut bad = png.to_vec();
    bad.push(0);
    put(source, bad);
    run.reject(
        &["approve", "--yes", "--revision", &rev],
        "Manager image digest changed",
    )
    .await;
    assert_eq!(run.snapshot().await["goals"], json!([]));
    put(source, png);
    cli_stdout(&run.manager(&["approve", "--yes", "--revision", &rev]).await);
    let after = wait_for(&run.client, &run.origin, &run.token, run.repo, |state| {
        let integrated = state["goals"][0]["execution"]["attempts"]
            .as_array()
            .is_some_and(|attempts| {
                attempts.len() == 2 && attempts.iter().all(|a| a["integrated"] == true)
            });
        assert!(
            integrated || state["goals"][0]["status"] != "blocked",
            "{}\n{}",
            state["goals"][0]["reason"],
            String::from_utf8_lossy(&bytes(&home.join("state/cruise/daemon/daemon.log")))
        );
        integrated
    })
    .await;
    let goal = &after["goals"][0];
    let task = string(&goal["execution"]["attempts"][0]["session_id"]);
    let other = string(&goal["execution"]["attempts"][1]["session_id"]);
    let session = home.join("data/cruise/sessions");
    assert_eq!(
        bytes(
            &session
                .join(task)
                .join("attachments")
                .join(format!("{id}.png"))
        ),
        png
    );
    assert!(!session.join(other).join("attachments").exists());
    assert_eq!(bytes(&proof), png);
    println!(
        "image attachment: {} bytes sha256 {}; same after restart and approved task read; unassigned task has no attachment",
        png.len(),
        digest(png)
    );
}

#[tokio::test]
async fn manager_image_turn_can_retry_only_before_provider_start() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let png = STANDARD
        .decode(PNG)
        .unwrap_or_else(|error| panic!("decode PNG fixture: {error}"));
    let file = home.join("sample.png");
    put(&file, &png);
    let (origin, token) = start(home, false);
    let stop = Stop(home);
    let output = ManagerCli::spawn(
        home,
        &repo,
        &[
            "say",
            "--image",
            file.to_str().unwrap_or_default(),
            "Not sent",
        ],
        None,
    )
    .finish()
    .await;
    assert!(!output.status.success());
    let state = snapshot(&daemon_client(), &origin, &token, &repo).await;
    let turn = &state["transcript"][0];
    assert_eq!(turn["image_started"], false);
    assert_eq!(turn["images"][0]["sha256"], digest(&png));
    drop(stop);

    let run = Run::start(home, &repo);
    let _stop = Stop(home);
    let provider = home.join("provider-image.png");
    assert!(!provider.exists());
    put(&home.join("release-jcode"), "capture-image");
    let retry = ["say", "--retry-image-turn", string(&turn["id"])];
    cli_stdout(&run.manager(&retry).await);
    let state = run.snapshot().await;
    assert_eq!(state["transcript"][0]["id"], turn["id"]);
    assert_eq!(state["transcript"][0]["image_started"], true);
    assert_eq!(state["transcript"][0]["status"], "completed");
    assert_eq!(bytes(&provider), png);
    run.reject(&retry, "not eligible to retry").await;
}
