#![cfg(unix)]

//! Contract tests for `cruise workflow add/update/remove/list`.
//!
//! GitHub is faked by a `gh` executable at the front of `PATH`; no network is used.

#[path = "support/workflow_packages.rs"]
mod workflow_packages_fixture;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const SHA_1: &str = "1111111111111111111111111111111111111111";
const SHA_2: &str = "2222222222222222222222222222222222222222";

const ROOT_YAML: &str = "command: [evil-llm]
steps:
  a:
    prompt_file: prompts/p.md
  b:
    command: echo side-effect-marker
  shared:
    workflow_call: ./shared.yaml
";

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        for d in ["bin", "home", "config", "cwd"] {
            std::fs::create_dir_all(root.path().join(d)).unwrap_or_else(|e| panic!("{e}"));
        }
        workflow_packages_fixture::install_gh(root.path());
        let fx = Self { root };
        fx.publish(SHA_1, ROOT_YAML, "PROMPT BODY ONE\n");
        fx
    }

    /// Publish a new upstream commit and make it the current branch head.
    fn publish(&self, sha: &str, root_yaml: &str, prompt: &str) {
        let base = self.root.path().join("bin/files").join(sha);
        std::fs::create_dir_all(base.join("prompts")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(base.join("cruise.yaml"), root_yaml).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(base.join("prompts/p.md"), prompt).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            base.join("shared.yaml"),
            "steps:\n  inner:\n    prompt: inner prompt body\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(self.root.path().join("bin/sha"), sha).unwrap_or_else(|e| panic!("{e}"));
    }

    fn publish_bundle(&self, sha: &str, manifest: &str, prompt: &str) {
        self.publish(sha, ROOT_YAML, prompt);
        let base = self.root.path().join("bin/files").join(sha);
        std::fs::create_dir_all(base.join("workflows")).unwrap();
        std::fs::write(base.join("cruise-package.yaml"), manifest).unwrap();
        std::fs::write(
            base.join("workflows/review.yaml"),
            "steps:\n  review:\n    prompt_file: ../prompts/p.md\n",
        )
        .unwrap();
        std::fs::write(
            base.join("workflows/implement.yaml"),
            "steps:\n  implement:\n    workflow_call: ../shared.yaml\n",
        )
        .unwrap();
    }

    fn cruise(&self, args: &[&str]) -> Output {
        self.cruise_env(args, &[])
    }

    fn cruise_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(name);
            }
        }
        let path = format!(
            "{}:{}",
            self.root.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.args(args)
            .current_dir(self.root.path().join("cwd"))
            .env("PATH", path)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap_or_else(|e| panic!("{e}"))
    }

    fn workflows(&self) -> PathBuf {
        self.root.path().join("config/cruise/workflows")
    }

    fn yaml(&self, name: &str) -> PathBuf {
        self.workflows().join(format!("{name}.yaml"))
    }

    fn manifest(&self, name: &str) -> PathBuf {
        self.workflows().join(format!("{name}.cruise-package.json"))
    }

    fn install(&self, name: &str) {
        let out = self.cruise(&["workflow", "add", "org/repo@main", "--name", name, "--yes"]);
        assert!(out.status.success(), "install failed: {}", text(&out));
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn add_without_yes_in_non_tty_writes_nothing() {
    let fx = Fixture::new();
    let out = fx.cruise(&["workflow", "add", "org/repo@main", "--name", "pkg"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!fx.yaml("pkg").exists());
    assert!(!fx.manifest("pkg").exists());
}

#[test]
fn add_with_yes_still_previews_sha_and_commands_and_pins_content() {
    let fx = Fixture::new();
    let out = fx.cruise(&["workflow", "add", "org/repo@main", "--name", "pkg", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(shown.contains(SHA_1), "preview lacks commit SHA: {shown}");
    assert!(shown.contains("evil-llm"), "preview lacks executor command");
    assert!(
        shown.contains("side-effect-marker"),
        "preview lacks step command"
    );

    let yaml = read(&fx.yaml("pkg"));
    assert!(yaml.contains("PROMPT BODY ONE"));
    assert!(yaml.contains("inner prompt body"));
    for forbidden in [
        "workflow_call",
        "prompt_file",
        "github.com",
        "githubusercontent",
    ] {
        assert!(
            !yaml.contains(forbidden),
            "saved yaml still has {forbidden}"
        );
    }

    let manifest: serde_json::Value =
        serde_json::from_str(&read(&fx.manifest("pkg"))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(manifest["name"], "pkg");
    assert_eq!(manifest["owner"], "org");
    assert_eq!(manifest["repo"], "repo");
    assert_eq!(manifest["workflow_path"], "cruise.yaml");
    assert_eq!(manifest["requested_ref"], "main");
    assert_eq!(manifest["resolved_commit"], SHA_1);
    assert_eq!(manifest["workflow_sha256"].as_str().map(str::len), Some(64));
}

#[test]
fn add_defaults_name_to_repo_for_root_workflow_and_does_not_run_it() {
    let fx = Fixture::new();
    let out = fx.cruise(&["workflow", "add", "org/repo", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(fx.yaml("repo").exists());
    // `command: echo side-effect-marker` must not have been executed.
    assert!(!fx.root.path().join("cwd/side-effect-marker").exists());
}

#[test]
fn installed_input_bearing_dependencies_remain_usable_without_upstream() {
    let fx = Fixture::new();
    fx.publish(
        SHA_1,
        "create_pr: false\nsteps:\n  follow-up:\n    workflow_call: ./_shared/after-pr.yaml\n    with:\n      message: 'installed {input}'\n",
        "unused\n",
    );
    let shared = fx.root.path().join("bin/files").join(SHA_1).join("_shared");
    std::fs::create_dir_all(&shared).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        shared.join("after-pr.yaml"),
        "inputs: [message]\nsteps:\n  report:\n    prompt_file: ../prompts/p.md\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        fx.root
            .path()
            .join("bin/files")
            .join(SHA_1)
            .join("prompts/p.md"),
        "Bound dependency: {inputs.message}\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    fx.install("bound");

    // Remove all remote contents, not just the root, before consuming the artifact.
    std::fs::remove_dir_all(fx.root.path().join("bin/files"))
        .unwrap_or_else(|error| panic!("{error}"));
    let yaml = read(&fx.yaml("bound"));
    let artifact: serde_yaml::Value =
        serde_yaml::from_str(&yaml).unwrap_or_else(|error| panic!("{error}"));
    assert!(artifact.get("inputs").is_none());
    let step = &artifact["steps"]["follow-up/report"];
    for field in ["workflow_call", "with", "prompt_file"] {
        assert!(step.get(field).is_none(), "installed step retains {field}");
    }
    let config = cruise::workflow_call::resolve_workflow_calls_without_app_model_defaults(
        cruise::config::WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|error| panic!("{error}")),
        fx.workflows(),
        &cruise::workflow_defaults::WorkflowDefaults::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        config.steps["follow-up/report"].prompt.as_deref(),
        Some("Bound dependency: installed {input}\n")
    );
    let out = fx.cruise(&[
        "exec",
        "--dry-run",
        "-c",
        fx.yaml("bound")
            .to_str()
            .unwrap_or_else(|| panic!("package path is not UTF-8")),
        "package-portability",
    ]);
    assert!(
        out.status.success(),
        "installed package unusable: {}",
        text(&out)
    );
}

#[test]
fn add_rejects_unsafe_name() {
    let fx = Fixture::new();
    let out = fx.cruise(&["workflow", "add", "org/repo", "--name", "../x", "--yes"]);
    assert!(!out.status.success());
    assert!(
        !fx.workflows().exists()
            || std::fs::read_dir(fx.workflows()).map_or(true, |d| d.count() == 0)
    );
}

#[test]
fn add_does_not_pin_environment_overrides() {
    let fx = Fixture::new();
    let out = fx.cruise_env(
        &["workflow", "add", "org/repo", "--name", "pkg", "--yes"],
        &[("CRUISE_MODEL", "env-model-zzz")],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(!read(&fx.yaml("pkg")).contains("env-model-zzz"));
}

#[test]
fn add_rejects_manual_yaml_and_yml_collisions_without_touching_them() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.workflows()).unwrap_or_else(|e| panic!("{e}"));
    let yaml = fx.yaml("same");
    let other_yml = fx.workflows().join("other.yml");
    std::fs::write(&yaml, "manual-yaml\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&other_yml, "manual-yml\n").unwrap_or_else(|e| panic!("{e}"));

    for name in ["same", "other"] {
        let out = fx.cruise(&["workflow", "add", "org/repo", "--name", name, "--yes"]);
        assert!(!out.status.success(), "{name}: {}", text(&out));
    }
    assert_eq!(read(&yaml), "manual-yaml\n");
    assert_eq!(read(&other_yml), "manual-yml\n");
    assert!(!fx.manifest("same").exists());
}

#[test]
fn add_rejects_existing_managed_name_without_changing_it() {
    let fx = Fixture::new();
    fx.install("pkg");
    let before = (read(&fx.yaml("pkg")), read(&fx.manifest("pkg")));
    fx.publish(SHA_2, ROOT_YAML, "PROMPT BODY TWO\n");
    let out = fx.cruise(&["workflow", "add", "org/repo", "--name", "pkg", "--yes"]);
    assert!(!out.status.success());
    assert_eq!(before, (read(&fx.yaml("pkg")), read(&fx.manifest("pkg"))));
}

#[test]
fn list_shows_installed_provenance_sorted_and_ignores_manual_and_broken_manifests() {
    let fx = Fixture::new();
    fx.install("zeta");
    fx.install("alpha");
    std::fs::write(fx.yaml("manual"), "steps: {}\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(fx.workflows().join("broken.yaml"), "steps: {}\n")
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(fx.manifest("broken"), "{not json").unwrap_or_else(|e| panic!("{e}"));

    let out = fx.cruise(&["workflow", "list"]);
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    let a = shown.find("alpha").unwrap_or_else(|| panic!("{shown}"));
    let z = shown.find("zeta").unwrap_or_else(|| panic!("{shown}"));
    assert!(a < z, "not name-sorted: {shown}");
    assert!(shown.contains("org/repo"));
    assert!(shown.contains("main"));
    assert!(shown.contains(&SHA_1[..7]));
    assert!(
        !shown.contains(SHA_1),
        "full SHA must be shortened: {shown}"
    );
    assert!(shown.contains("alpha.yaml"));
    assert!(!shown.contains("manual"));
    assert!(!shown.contains("broken"));
}

#[test]
fn update_follows_recorded_ref_and_replaces_pair() {
    let fx = Fixture::new();
    fx.install("pkg");
    fx.publish(SHA_2, ROOT_YAML, "PROMPT BODY TWO\n");
    let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(SHA_2));
    assert!(read(&fx.yaml("pkg")).contains("PROMPT BODY TWO"));
    let manifest: serde_json::Value =
        serde_json::from_str(&read(&fx.manifest("pkg"))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(manifest["resolved_commit"], SHA_2);
    assert_eq!(manifest["requested_ref"], "main");
}

#[test]
fn update_without_confirmation_or_with_invalid_upstream_keeps_old_pair() {
    let fx = Fixture::new();
    fx.install("pkg");
    let before = (read(&fx.yaml("pkg")), read(&fx.manifest("pkg")));

    fx.publish(SHA_2, ROOT_YAML, "PROMPT BODY TWO\n");
    let out = fx.cruise(&["workflow", "update", "pkg"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(before, (read(&fx.yaml("pkg")), read(&fx.manifest("pkg"))));

    fx.publish(SHA_2, "steps: [not, a, map\n", "x\n");
    let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(before, (read(&fx.yaml("pkg")), read(&fx.manifest("pkg"))));
}

#[test]
fn update_and_remove_refuse_hand_edited_package() {
    let fx = Fixture::new();
    fx.install("pkg");
    let edited = format!("{}# edited\n", read(&fx.yaml("pkg")));
    std::fs::write(fx.yaml("pkg"), &edited).unwrap_or_else(|e| panic!("{e}"));
    let manifest_before = read(&fx.manifest("pkg"));
    fx.publish(SHA_2, ROOT_YAML, "PROMPT BODY TWO\n");

    let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("Modified") || text(&out).to_lowercase().contains("modified"));
    let out = fx.cruise(&["workflow", "remove", "pkg"]);
    assert!(!out.status.success());
    assert_eq!(read(&fx.yaml("pkg")), edited);
    assert_eq!(read(&fx.manifest("pkg")), manifest_before);
}

#[test]
fn remove_deletes_only_the_managed_pair() {
    let fx = Fixture::new();
    fx.install("pkg");
    let manual = fx.yaml("manual");
    let neighbour = fx.workflows().join("pkg.notes.txt");
    std::fs::write(&manual, "steps: {}\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&neighbour, "keep\n").unwrap_or_else(|e| panic!("{e}"));

    let out = fx.cruise(&["workflow", "remove", "pkg"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!fx.yaml("pkg").exists());
    assert!(!fx.manifest("pkg").exists());
    assert!(manual.exists());
    assert!(neighbour.exists());
    assert!(!text(&fx.cruise(&["workflow", "list"])).contains("pkg"));
}

#[test]
fn remove_refuses_unmanaged_yaml_and_unknown_names() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.workflows()).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(fx.yaml("manual"), "steps: {}\n").unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !fx.cruise(&["workflow", "remove", "manual"])
            .status
            .success()
    );
    assert!(fx.yaml("manual").exists());
    assert!(!fx.cruise(&["workflow", "remove", "ghost"]).status.success());
}

const BUNDLE_MANIFEST: &str = "description: Example bundle\nworkflows:\n  - name: review\n    path: workflows/review.yaml\n  - name: implement\n    path: workflows/implement.yaml\n";

#[test]
fn bundle_preview_keeps_commands_after_warning_line() {
    let fx = Fixture::new();
    fx.publish_bundle(SHA_1, BUNDLE_MANIFEST, "prompt\n");
    std::fs::write(
        fx.root.path().join(format!("bin/files/{SHA_1}/workflows/review.yaml")),
        "steps:\n  first:\n    command: |\n      echo first\n      WARNING: embedded line\n  second:\n    command: echo after-warning-marker\n",
    ).unwrap();
    let out = fx.cruise(&["workflow", "add", "org/repo@main", "--name", "pkg"]);
    let preview = text(&out);
    assert!(!out.status.success());
    assert!(preview.contains("WARNING: embedded line"), "{preview}");
    assert!(preview.contains("after-warning-marker"), "{preview}");
    assert!(!fx.workflows().join("pkg").exists());
}

#[test]
fn single_package_update_and_remove_preserve_same_named_directory() {
    let fx = Fixture::new();
    fx.install("pkg");
    let unrelated = fx.workflows().join("pkg");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep"), "untouched").unwrap();
    fx.publish(SHA_2, ROOT_YAML, "UPDATED PROMPT\n");
    let update = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(update.status.success(), "{}", text(&update));
    assert!(read(&fx.yaml("pkg")).contains("UPDATED PROMPT"));
    let remove = fx.cruise(&["workflow", "remove", "pkg"]);
    assert!(remove.status.success(), "{}", text(&remove));
    assert_eq!(read(&unrelated.join("keep")), "untouched");
}

#[test]
fn bundle_install_preview_lock_and_self_contained_workflows() {
    let fx = Fixture::new();
    fx.publish_bundle(SHA_1, BUNDLE_MANIFEST, "BUNDLE PROMPT\n");
    let declined = fx.cruise(&["workflow", "add", "org/repo@main", "--name", "pkg"]);
    assert!(!declined.status.success(), "{}", text(&declined));
    assert!(!fx.workflows().join("pkg").exists());
    let out = fx.cruise(&["workflow", "add", "org/repo@main", "--name", "pkg", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    let preview = text(&out);
    for expected in ["review", "implement", "WARNING", SHA_1] {
        assert!(preview.contains(expected), "missing {expected}: {preview}");
    }
    for name in ["review", "implement"] {
        assert!(preview.contains(&format!("WARNING: workflow {name} is untrusted")), "{preview}");
    }
    let dir = fx.workflows().join("pkg");
    let review = read(&dir.join("review.yaml"));
    assert!(review.contains("BUNDLE PROMPT"));
    let implement = read(&dir.join("implement.yaml"));
    assert!(implement.contains("inner prompt body"));
    for yaml in [&review, &implement] {
        for forbidden in [
            "prompt_file",
            "workflow_call",
            "github.com",
            "githubusercontent",
        ] {
            assert!(!yaml.contains(forbidden), "artifact retains {forbidden}");
        }
    }
    assert!(!dir.join("shared.yaml").exists());
    let lock: serde_json::Value =
        serde_json::from_str(&read(&dir.join(".cruise-package-lock.json"))).unwrap();
    assert_eq!(lock["resolved_commit"], SHA_1);
    assert_eq!(
        lock["workflows"]["review"]["source_path"],
        "workflows/review.yaml"
    );
    use sha2::{Digest as _, Sha256};
    for name in ["review", "implement"] {
        let bytes = std::fs::read(dir.join(format!("{name}.yaml"))).unwrap();
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(lock["workflows"][name]["sha256"], digest);
    }
    assert!(text(&fx.cruise(&["workflow", "list"])).contains("pkg"));
}

#[test]
fn bundle_invalid_manifests_leave_no_install() {
    for manifest in [
        "workflows: []\n",
        "workflows:\n  - name: review\n    path: ../outside.yaml\n",
        "workflows:\n  - name: review\n    path: workflows/review.yaml\n  - name: REVIEW\n    path: workflows/implement.yaml\n",
        "workflows:\n  - name: review\n    path: workflows/review.yaml\n  - name: other\n    path: workflows/review.yaml\n",
        "path: workflows\nworkflows:\n  - name: review\n    path: workflows/review.yaml\n",
    ] {
        let fx = Fixture::new();
        fx.publish_bundle(SHA_1, manifest, "prompt\n");
        let out = fx.cruise(&["workflow", "add", "org/repo@main", "--name", "pkg", "--yes"]);
        assert!(!out.status.success(), "accepted {manifest}: {}", text(&out));
        assert!(!fx.workflows().join("pkg").exists());
    }
}

#[test]
fn bundle_update_replaces_dropped_and_new_members() {
    let fx = Fixture::new();
    fx.publish_bundle(SHA_1, BUNDLE_MANIFEST, "old prompt\n");
    fx.install("pkg");
    let dir = fx.workflows().join("pkg");
    let manifest = "workflows:\n  - name: review\n    path: workflows/review.yaml\n  - name: deploy\n    path: workflows/deploy.yaml\n";
    fx.publish_bundle(SHA_2, manifest, "new prompt\n");
    std::fs::write(
        fx.root.path().join(format!("bin/files/{SHA_2}/workflows/deploy.yaml")),
        "steps:\n  deploy:\n    prompt: new deployment\n",
    ).unwrap();
    let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!dir.join("implement.yaml").exists());
    assert!(read(&dir.join("review.yaml")).contains("new prompt"));
    assert!(read(&dir.join("deploy.yaml")).contains("new deployment"));
    let lock: serde_json::Value = serde_json::from_str(&read(&dir.join(".cruise-package-lock.json"))).unwrap();
    assert_eq!(lock["resolved_commit"], SHA_2);
    let members = lock["workflows"].as_object().unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.contains_key("review"));
    assert!(members.contains_key("deploy"));
    assert!(!members.contains_key("implement"));
}

#[test]
fn bundle_missing_or_nonregular_listed_file_preserves_install_and_update() {
    for mode in [None, Some("blob 120000"), Some("tree 040000")] {
        let fx = Fixture::new();
        fx.publish_bundle(SHA_1, BUNDLE_MANIFEST, "old prompt\n");
        let upstream = fx.root.path().join(format!("bin/files/{SHA_1}/workflows/implement.yaml"));
        if let Some(mode) = mode {
            let mode_path = fx.root.path().join(format!("bin/modes/{SHA_1}/workflows/implement.yaml"));
            std::fs::create_dir_all(mode_path.parent().unwrap()).unwrap();
            std::fs::write(mode_path, mode).unwrap();
        } else {
            std::fs::remove_file(&upstream).unwrap();
        }
        let out = fx.cruise(&["workflow", "add", "org/repo", "--name", "pkg", "--yes"]);
        assert!(!out.status.success(), "mode {mode:?}: {}", text(&out));
        assert!(!fx.workflows().join("pkg").exists());

        fx.publish_bundle(SHA_2, BUNDLE_MANIFEST, "installed prompt\n");
        fx.install("pkg");
        let dir = fx.workflows().join("pkg");
        let files = ["review.yaml", "implement.yaml", ".cruise-package-lock.json"];
        let before: Vec<_> = files.iter().map(|name| std::fs::read(dir.join(name)).unwrap()).collect();
        std::fs::write(fx.root.path().join("bin/sha"), SHA_1).unwrap();
        let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
        assert!(!out.status.success(), "mode {mode:?}: {}", text(&out));
        let after: Vec<_> = files.iter().map(|name| std::fs::read(dir.join(name)).unwrap()).collect();
        assert_eq!(before, after);
    }
}

#[test]
fn bundle_update_is_atomic_and_remove_protects_unmanaged_files() {
    let fx = Fixture::new();
    fx.publish_bundle(SHA_1, BUNDLE_MANIFEST, "OLD PROMPT\n");
    fx.install("pkg");
    let dir = fx.workflows().join("pkg");
    let review = dir.join("review.yaml");
    let lock = dir.join(".cruise-package-lock.json");
    let before = (read(&review), read(&lock));
    fx.publish_bundle(SHA_2, "workflows: []\n", "NEW PROMPT\n");
    let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(before, (read(&review), read(&lock)));
    fx.publish_bundle(SHA_2, BUNDLE_MANIFEST, "NEW PROMPT\n");
    let out = fx.cruise(&["workflow", "update", "pkg", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(read(&review).contains("NEW PROMPT"));
    assert!(read(&lock).contains(SHA_2));
    std::fs::write(dir.join("notes.txt"), "keep").unwrap();
    assert!(!fx.cruise(&["workflow", "remove", "pkg"]).status.success());
    assert!(dir.join("notes.txt").exists());
    std::fs::remove_file(dir.join("notes.txt")).unwrap();
    let out = fx.cruise(&["workflow", "remove", "pkg"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!dir.exists());
}
