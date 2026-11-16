//! Executable provider installation contracts through the production CLI.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

#[cfg(unix)]
mod contracts {
    use cucumber::{World as _, given, then, when};
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::Path,
        process::{Command, Output},
        time::UNIX_EPOCH,
    };

    #[derive(Debug, Default, cucumber::World)]
    pub struct InstallWorld {
        root: Option<tempfile::TempDir>,
        output: Option<Output>,
        missing_override: bool,
        valid_override: bool,
    }

    impl InstallWorld {
        fn root(&self) -> &std::path::Path {
            self.root.as_ref().unwrap().path()
        }
        fn run(&mut self, args: &[&str]) {
            let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
            command
                .env_clear()
                .current_dir(self.root())
                .env("HOME", self.root().join("home"))
                .env("XDG_DATA_HOME", self.root().join("data"))
                .env("XDG_CONFIG_HOME", self.root().join("config"))
                .env("XDG_STATE_HOME", self.root().join("state"))
                .env("PATH", self.root().join("bin"))
                .env("HTTP_PROXY", "http://127.0.0.1:1")
                .env("HTTPS_PROXY", "http://127.0.0.1:1")
                .args(args);
            if self.missing_override {
                command.env("CRUISE_JCODE_CLI_PATH", self.root().join("missing"));
            } else if self.valid_override {
                command.env("CRUISE_JCODE_CLI_PATH", self.root().join("override"));
            }
            self.output = Some(command.output().unwrap());
        }
        fn output(&self) -> &Output {
            self.output.as_ref().unwrap()
        }
        fn text(&self) -> String {
            format!(
                "{}{}",
                String::from_utf8_lossy(&self.output().stdout),
                String::from_utf8_lossy(&self.output().stderr)
            )
        }
    }

    #[given("isolated provider binaries on PATH")]
    fn setup(w: &mut InstallWorld) {
        w.root = Some(tempfile::tempdir().unwrap());
        fs::create_dir(w.root().join("bin")).unwrap();
        for provider in ["jcode", "claude", "opencode"] {
            let path = w.root().join("bin").join(provider);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho invoked >> '{}'\n",
                    w.root().join("executed").display()
                ),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fn recorded_jcode(root: &Path, version: &str) {
        let dir = root.join("data/cruise/providers/jcode");
        let relative_path = format!("{version}/jcode");
        let binary = dir.join(&relative_path);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\necho invoked >> '{}'\n",
                root.join("executed").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let metadata = fs::metadata(&binary).unwrap();
        let mtime_unix_nanos = metadata
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let target = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "macos-aarch64",
            ("macos", "x86_64") => "macos-x86_64",
            ("linux", "aarch64") if !cfg!(target_env = "musl") => "linux-aarch64",
            ("linux", "x86_64") if !cfg!(target_env = "musl") => "linux-x86_64",
            unsupported => panic!("unsupported jcode fixture target: {unsupported:?}"),
        };
        let record = serde_json::json!({
            "schema": 1,
            "provider": "jcode",
            "version": version,
            "target": target,
            "relative_path": relative_path,
            "sha256": "0".repeat(64),
            "len": metadata.len(),
            "mtime_unix_nanos": mtime_unix_nanos,
            "source_url": "https://example.invalid/jcode"
        });
        fs::write(
            dir.join("install.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    fn jcode_row(w: &InstallWorld) -> String {
        w.text()
            .lines()
            .find(|line| line.split_whitespace().next() == Some("jcode"))
            .unwrap()
            .to_owned()
    }

    #[when("I run provider status")]
    fn status(w: &mut InstallWorld) {
        w.run(&["install", "--status"]);
    }
    #[given("a valid older recorded jcode installation")]
    fn older_record(w: &mut InstallWorld) {
        recorded_jcode(w.root(), "0.1.0");
    }
    #[given("the recorded jcode binary has been damaged")]
    fn damaged_record(w: &mut InstallWorld) {
        recorded_jcode(w.root(), "0.1.0");
        fs::write(
            w.root().join("data/cruise/providers/jcode/0.1.0/jcode"),
            b"damaged binary",
        )
        .unwrap();
    }
    #[given("the jcode override points to a valid file")]
    fn valid_override(w: &mut InstallWorld) {
        let path = w.root().join("override");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho invoked >> '{}'\n",
                w.root().join("executed").display()
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        w.valid_override = true;
    }
    #[given("the jcode override points to a missing file")]
    fn missing_override(w: &mut InstallWorld) {
        w.missing_override = true;
    }
    #[when(regex = r"^I run install with (.+)$")]
    fn install(w: &mut InstallWorld, captures: &[String]) {
        let mut command = vec!["install"];
        command.extend(captures[0].split_whitespace());
        w.run(&command);
    }
    #[then("status reports no installed versions and uses PATH")]
    fn versions(w: &mut InstallWorld) {
        assert!(w.output().status.success(), "{}", w.text());
        let text = w.text();
        for value in ["VERSION", "EFFECTIVE"] {
            assert!(text.contains(value), "missing {value}: {text}");
        }
        for provider in ["jcode", "claude", "opencode"] {
            let row = text
                .lines()
                .find(|line| line.split_whitespace().next() == Some(provider))
                .unwrap();
            assert_eq!(row.split_whitespace().nth(1), Some("-"), "{row}");
            assert_eq!(row.split_whitespace().last(), Some("path"), "{row}");
        }
    }
    #[then("status reports the recorded older jcode version as managed")]
    fn older_version_is_managed(w: &mut InstallWorld) {
        assert!(w.output().status.success(), "{}", w.text());
        let row = jcode_row(w);
        assert_eq!(row.split_whitespace().nth(1), Some("0.1.0"), "{row}");
        assert!(row.contains("ready"), "{row}");
        assert_eq!(row.split_whitespace().last(), Some("managed"), "{row}");
    }
    #[then("status reports the damaged jcode install and falls back to PATH")]
    fn damaged_falls_back(w: &mut InstallWorld) {
        assert!(w.output().status.success(), "{}", w.text());
        let row = jcode_row(w);
        assert!(row.contains("damaged"), "{row}");
        assert_eq!(row.split_whitespace().last(), Some("path"), "{row}");
    }
    #[then("status reports the jcode override as effective")]
    fn override_effective(w: &mut InstallWorld) {
        assert!(w.output().status.success(), "{}", w.text());
        let row = jcode_row(w);
        assert!(
            row.contains(w.root().join("override").to_str().unwrap()),
            "{row}"
        );
        assert_eq!(row.split_whitespace().last(), Some("override"), "{row}");
    }
    #[then("the install command fails")]
    fn fails(w: &mut InstallWorld) {
        assert!(!w.output().status.success(), "{}", w.text());
    }
    #[then("the jcode status reports an override error")]
    fn override_error(w: &mut InstallWorld) {
        let row = jcode_row(w);
        assert!(
            row.contains(w.root().join("missing").to_str().unwrap()),
            "{row}"
        );
        assert_eq!(row.split_whitespace().last(), Some("error"), "{row}");
    }
    #[then("no provider ran")]
    fn no_provider_ran(w: &mut InstallWorld) {
        assert!(!w.root().join("executed").exists());
    }
    #[then("no provider ran and no data directory was created")]
    fn no_side_effects(w: &mut InstallWorld) {
        no_provider_ran(w);
        assert!(!w.root().join("data").exists());
    }
    pub async fn main() {
        use cucumber::writer::Stats as _;
        let writer = InstallWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/provider_install.feature"
            ))
            .await;
        assert!(
            !writer.execution_has_failed(),
            "{} failed steps, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
        assert!(
            writer.passed_steps() + writer.failed_steps() > 0,
            "no provider scenarios executed"
        );
    }
}
#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::main().await;
}
#[cfg(not(unix))]
fn main() {
    panic!("provider_install_bdd requires Unix subprocess fixtures");
}
