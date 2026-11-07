//! Executable provider installation contracts through the production CLI.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

#[cfg(unix)]
mod contracts {
    use cucumber::{World as _, given, then, when};
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        process::{Command, Output},
    };

    #[derive(Debug, Default, cucumber::World)]
    pub struct InstallWorld {
        root: Option<tempfile::TempDir>,
        output: Option<Output>,
        missing_override: bool,
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
    #[given("the jcode override points to a missing file")]
    fn missing_override(w: &mut InstallWorld) {
        w.missing_override = true;
    }
    #[when("I run provider status")]
    fn status(w: &mut InstallWorld) {
        w.run(&["install", "--status"]);
    }
    #[when(regex = r"^I run install with (.+)$")]
    fn install(w: &mut InstallWorld, args: String) {
        let mut command = vec!["install"];
        command.extend(args.split_whitespace());
        w.run(&command);
    }
    #[then("status reports all pinned providers")]
    fn pins(w: &mut InstallWorld) {
        assert!(w.output().status.success(), "{}", w.text());
        let text = w.text();
        for value in [
            "jcode",
            "claude",
            "opencode",
            "0.99.1",
            "2.1.250",
            "2.1.3",
            "EFFECTIVE",
        ] {
            assert!(text.contains(value), "missing {value}: {text}");
        }
    }
    #[then("the install command fails")]
    fn fails(w: &mut InstallWorld) {
        assert!(!w.output().status.success(), "{}", w.text());
    }
    #[then("the jcode status reports an override error")]
    fn override_error(w: &mut InstallWorld) {
        let text = w.text();
        let row = text
            .lines()
            .find(|line| line.split_whitespace().next() == Some("jcode"))
            .unwrap();
        assert!(
            row.contains(w.root().join("missing").to_str().unwrap()),
            "{row}"
        );
        assert_eq!(row.split_whitespace().last(), Some("error"), "{row}");
    }
    #[then("no provider ran and no data directory was created")]
    fn no_side_effects(w: &mut InstallWorld) {
        assert!(!w.root().join("executed").exists());
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
