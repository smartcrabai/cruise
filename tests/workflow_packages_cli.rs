#![cfg(unix)]

//! Contract tests for `cruise workflow add/update/remove/list`.
//!
//! GitHub is faked by a `gh` executable at the front of `PATH`; no network is used.

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

const GH_STUB: &str = r#"#!/bin/sh
D="$(dirname "$0")"
args="$*"
case "$args" in
  *contents/*)
    path="${args#*contents/}"; path="${path%%\?*}"
    ref="${args##*ref=}"; ref="${ref%% *}"
    sha="$(cat "$D/sha")"
    [ "$ref" = "$sha" ] || { echo "contents requested with unpinned ref $ref" >&2; exit 1; }
    f="$D/files/$sha/$path"
    [ -f "$f" ] || { echo "not found" >&2; exit 1; }
    base64 < "$f" | tr -d '\n'; echo ;;
  *commits/*) cat "$D/sha" ;;
  *) case "$args" in *default_branch*) echo main ;; *) echo "unexpected: $args" >&2; exit 1 ;; esac ;;
esac
"#;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        for d in ["bin", "home", "config", "cwd"] {
            std::fs::create_dir_all(root.path().join(d)).unwrap_or_else(|e| panic!("{e}"));
        }
        let gh = root.path().join("bin/gh");
        std::fs::write(&gh, GH_STUB).unwrap_or_else(|e| panic!("{e}"));
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|e| panic!("{e}"));
        }
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
    let yml = fx.workflows().join("other.yml");
    std::fs::write(&yaml, "manual-yaml\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&yml, "manual-yml\n").unwrap_or_else(|e| panic!("{e}"));

    for name in ["same", "other"] {
        let out = fx.cruise(&["workflow", "add", "org/repo", "--name", name, "--yes"]);
        assert!(!out.status.success(), "{name}: {}", text(&out));
    }
    assert_eq!(read(&yaml), "manual-yaml\n");
    assert_eq!(read(&yml), "manual-yml\n");
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
    assert!(shown.contains(SHA_1));
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
