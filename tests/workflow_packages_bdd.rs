#![expect(clippy::unwrap_used, reason = "isolated BDD fixtures")]

#[cfg(unix)]
#[path = "support/workflow_packages.rs"]
mod workflow_packages_fixture;

#[cfg(unix)]
mod contracts {
    use cucumber::{World as _, given, then, when};
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};

    const SHA: &str = "1111111111111111111111111111111111111111";
    const SHA_2: &str = "2222222222222222222222222222222222222222";
    const MANIFEST: &str = "workflows:\n  - name: review\n    path: workflows/review.yaml\n  - name: implement\n    path: workflows/implement.yaml\n";

    #[derive(Debug, Default, cucumber::World)]
    struct PackageWorld {
        root: Option<tempfile::TempDir>,
        output: Option<Output>,
        before: Vec<Vec<u8>>,
    }

    impl PackageWorld {
        fn root(&self) -> PathBuf {
            self.root.as_ref().unwrap().path().to_path_buf()
        }
        fn workflows(&self) -> PathBuf {
            self.root().join("config/cruise/workflows")
        }
        fn package(&self) -> PathBuf {
            self.workflows().join("pkg")
        }
        fn command(&self, args: &[&str]) -> Output {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("CRUISE_") {
                    cmd.env_remove(key);
                }
            }
            cmd.args(args)
                .current_dir(self.root().join("cwd"))
                .env("HOME", self.root().join("home"))
                .env("XDG_CONFIG_HOME", self.root().join("config"))
                .env("XDG_STATE_HOME", self.root().join("state"))
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        self.root().join("bin").display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                )
                .env("NO_COLOR", "1")
                .stdin(Stdio::null())
                .output()
                .unwrap()
        }
        fn install(&self) -> Output {
            self.command(&["workflow", "add", "org/repo", "--name", "pkg", "--yes"])
        }
        fn snapshot(&self) -> Vec<Vec<u8>> {
            ["review.yaml", "implement.yaml", ".cruise-package-lock.json"]
                .iter()
                .map(|name| std::fs::read(self.package().join(name)).unwrap())
                .collect()
        }
    }
    fn success(out: &Output) {
        assert!(
            out.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    #[given("an isolated bundle repository")]
    fn repository(w: &mut PackageWorld) {
        let root = tempfile::tempdir().unwrap();
        for dir in ["bin/files/workflows", "home", "config", "cwd", "state"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        super::workflow_packages_fixture::install_gh(root.path());
        std::fs::write(root.path().join("bin/sha"), SHA).unwrap();
        std::fs::write(root.path().join("bin/files/cruise-package.yaml"), MANIFEST).unwrap();
        std::fs::write(
            root.path().join("bin/files/cruise.yaml"),
            "steps:\n  root:\n    prompt: root workflow\n",
        )
        .unwrap();
        for name in ["review", "implement"] {
            std::fs::write(
                root.path().join(format!("bin/files/workflows/{name}.yaml")),
                format!("command: [echo]\nsteps:\n  {name}:\n    command: echo {name}\n"),
            )
            .unwrap();
        }
        w.root = Some(root);
    }
    #[given("an installed bundle")]
    fn installed(w: &mut PackageWorld) {
        repository(w);
        success(&w.install());
    }
    #[when("I install the bundle")]
    fn install(w: &mut PackageWorld) {
        w.output = Some(w.install());
    }
    #[then("both workflows and their lock are installed")]
    fn both(w: &mut PackageWorld) {
        success(w.output.as_ref().unwrap());
        for name in ["review.yaml", "implement.yaml", ".cruise-package-lock.json"] {
            assert!(w.package().join(name).is_file(), "missing {name}");
        }
        let lock: serde_json::Value = serde_json::from_slice(
            &std::fs::read(w.package().join(".cruise-package-lock.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(lock["workflows"].as_object().unwrap().len(), 2);
    }
    #[when("I install an invalid bundle")]
    fn invalid(w: &mut PackageWorld) {
        std::fs::write(
            w.root().join("bin/files/cruise-package.yaml"),
            "workflows: []\n",
        )
        .unwrap();
        w.output = Some(w.install());
    }
    #[then("no package is installed")]
    fn none(w: &mut PackageWorld) {
        assert!(!w.output.as_ref().unwrap().status.success());
        assert!(!w.package().exists());
        assert!(
            !w.workflows().exists() || std::fs::read_dir(w.workflows()).unwrap().next().is_none()
        );
    }
    #[when("an interrupted replacement is followed by an update")]
    fn interrupted(w: &mut PackageWorld) {
        std::fs::rename(w.package(), w.workflows().join(".pkg.previous-interrupted")).unwrap();
        std::fs::create_dir(w.workflows().join(".pkg.staging-interrupted")).unwrap();
        w.output = Some(w.command(&["workflow", "update", "pkg", "--yes"]));
    }
    #[then("the interrupted replacement is recovered")]
    fn recovered(w: &mut PackageWorld) {
        success(w.output.as_ref().unwrap());
        assert!(w.package().join("review.yaml").exists());
        assert!(w.package().join("implement.yaml").exists());
        assert!(!w.workflows().join(".pkg.previous-interrupted").exists());
        assert!(!w.workflows().join(".pkg.staging-interrupted").exists());
    }
    #[then("hand edits and unknown files prevent update and removal")]
    fn modifications(w: &mut PackageWorld) {
        let file = w.package().join("review.yaml");
        let original = std::fs::read(&file).unwrap();
        std::fs::write(&file, b"hand edited").unwrap();
        for args in [
            vec!["workflow", "update", "pkg", "--yes"],
            vec!["workflow", "remove", "pkg"],
        ] {
            assert!(
                !w.command(&args).status.success(),
                "{:?} accepted hand edit",
                args
            );
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"hand edited");
        std::fs::write(&file, original).unwrap();
        let extra = w.package().join("notes.txt");
        std::fs::write(&extra, b"keep me").unwrap();
        for args in [
            vec!["workflow", "update", "pkg", "--yes"],
            vec!["workflow", "remove", "pkg"],
        ] {
            assert!(
                !w.command(&args).status.success(),
                "{:?} accepted unknown file",
                args
            );
        }
        assert_eq!(std::fs::read(extra).unwrap(), b"keep me");
    }
    #[then("the bundle member is discoverable by its relative name")]
    fn discoverable(w: &mut PackageWorld) {
        let out = Command::new(std::env::current_exe().unwrap())
            .env("CRUISE_BDD_DISCOVERY", "1")
            .env("HOME", w.root().join("home"))
            .env("XDG_CONFIG_HOME", w.root().join("config"))
            .output()
            .unwrap();
        success(&out);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.lines().any(|line| line == "pkg/review.yaml"), "{text}");
    }
    #[then("the bundle member is selectable")]
    fn discovery(w: &mut PackageWorld) {
        let path = w.package().join("review.yaml");
        let out = w.command(&[
            "exec",
            "--dry-run",
            "-c",
            path.to_str().unwrap(),
            "bundle-test",
        ]);
        success(&out);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("review"), "{text}");
    }
    #[when("a new revision has an invalid manifest")]
    fn invalid_revision(w: &mut PackageWorld) {
        w.before = w.snapshot();
        std::fs::write(w.root().join("bin/sha"), SHA_2).unwrap();
        std::fs::write(
            w.root().join("bin/files/cruise-package.yaml"),
            "workflows: []\n",
        )
        .unwrap();
        w.output = Some(w.command(&["workflow", "update", "pkg", "--yes"]));
    }
    #[then("updating fails without changing either workflow or the lock")]
    fn unchanged(w: &mut PackageWorld) {
        assert!(!w.output.as_ref().unwrap().status.success());
        assert_eq!(w.snapshot(), w.before);
    }
    #[when("the new revision is repaired and updated")]
    fn repaired(w: &mut PackageWorld) {
        std::fs::write(w.root().join("bin/files/cruise-package.yaml"), MANIFEST).unwrap();
        for name in ["review", "implement"] {
            std::fs::write(
                w.root().join(format!("bin/files/workflows/{name}.yaml")),
                format!("command: [echo]\nsteps:\n  {name}:\n    command: echo updated-{name}\n"),
            )
            .unwrap();
        }
        w.output = Some(w.command(&["workflow", "update", "pkg", "--yes"]));
    }
    #[then("both workflows and their lock reflect the new revision")]
    fn updated(w: &mut PackageWorld) {
        use sha2::{Digest as _, Sha256};
        success(w.output.as_ref().unwrap());
        let lock: serde_json::Value = serde_json::from_slice(&w.snapshot()[2]).unwrap();
        assert_eq!(lock["resolved_commit"], SHA_2);
        for name in ["review", "implement"] {
            let bytes = std::fs::read(w.package().join(format!("{name}.yaml"))).unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains(&format!("updated-{name}")));
            let digest: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(lock["workflows"][name]["sha256"], digest);
        }
    }
    #[then("the unmodified bundle can be removed")]
    fn removed(w: &mut PackageWorld) {
        success(&w.command(&["workflow", "remove", "pkg"]));
        assert!(!w.package().exists());
    }
    pub async fn main() {
        PackageWorld::run("tests/features/workflow_packages.feature").await;
    }
}
#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if std::env::var_os("CRUISE_BDD_DISCOVERY").is_some() {
        for entry in cruise::configs::list_user_configs() {
            println!("{}", entry.name);
        }
        return;
    }
    contracts::main().await;
}
#[cfg(not(unix))]
fn main() {
    eprintln!("BLOCKED: workflow_packages_bdd requires Unix subprocess fixtures");
    std::process::exit(1);
}
