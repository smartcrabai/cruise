//! Run the same real CLI/daemon fixtures as the stand-alone Bats suite.
#[cfg(not(unix))]
fn main() {
    // Cargo's --all-targets can select this target even with test = false.
    // The dedicated POSIX runner is the CI check and rejects other platforms.
    eprintln!("Exec recovery BDD is not a Windows CI check");
}

#[cfg(unix)]
fn main() {
    posix::run();
}

#[cfg(unix)]
mod posix {
    use std::process::{Command, Output};

    use cucumber::{World as _, then, when, writer::Stats as _};

    const SCENARIOS: usize = 15;

    #[derive(Debug, Default, cucumber::World)]
    struct RecoveryWorld {
        fixture: Option<Output>,
    }

    #[when(regex = r#"^the production recovery fixture "([a-z-]+)" runs$"#)]
    async fn run_fixture(world: &mut RecoveryWorld, fixture: String) {
        assert!(world.fixture.is_none(), "fixture was already run");
        let binary =
            std::env::var_os("CRUISE_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_cruise").into());
        world.fixture = Some(
            Command::new("bats")
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .env("CRUISE_BIN", binary)
                .env_remove("BATS_FILTER_TAGS")
                .args(["--tap", "--print-output-on-failure", "--filter-tags"])
                .arg(format!("recovery-{fixture}"))
                .arg("tests/exec_recovery_cli.bats")
                .output()
                .unwrap_or_else(|error| {
                    panic!("could not run recovery fixture {fixture}: {error}")
                }),
        );
    }

    #[then("the production fixture passes all its recovery assertions")]
    async fn assert_fixture_passed(world: &mut RecoveryWorld) {
        let output = world
            .fixture
            .as_ref()
            .unwrap_or_else(|| panic!("no production fixture ran"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut lines = stdout.lines().filter(|line| !line.starts_with('#'));
        // Bats can exit successfully with zero tests or a skipped test. Neither is
        // evidence that the selected production assertions ran.
        assert!(
            output.status.success()
                && lines.next() == Some("1..1")
                && lines.next().is_some_and(|line| {
                    line.starts_with("ok 1 ") && !line.to_ascii_lowercase().contains(" # skip")
                })
                && lines.next().is_none(),
            "expected exactly one passing production Bats test, got {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status,
        );
    }

    #[tokio::main(flavor = "current_thread")]
    pub(super) async fn run() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/features/exec_recovery.feature");
        let writer = RecoveryWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(path)
            .await;
        assert!(
            !writer.execution_has_failed(),
            "exec recovery scenarios failed"
        );
        assert_eq!(
            writer.passed_steps(),
            SCENARIOS * 2,
            "incomplete recovery coverage"
        );
    }
}
