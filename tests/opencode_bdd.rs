//! Executable `OpenCode` contracts from plan sections 4.3, 5.2, 5.3 and 5.5.
//! Reuses the pinned HTTP/SSE and CLI-lifetime fixture, not a mock Executor.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

#[cfg(unix)]
#[expect(
    dead_code,
    unused_imports,
    reason = "shared integration fixture includes standalone tests"
)]
#[path = "opencode_sdk_cli.rs"]
mod fixture;

#[cfg(unix)]
mod contracts {
    use super::fixture::{ANSWER, Fixture, MODEL, Scenario};
    use cruise::executor::PromptOutcome;
    use cucumber::{World as _, given, then, when};

    #[derive(Default, cucumber::World)]
    pub struct OpenCodeWorld {
        case: String,
        fixture: Option<Fixture>,
        outcome: Option<cruise::error::Result<PromptOutcome>>,
    }

    impl std::fmt::Debug for OpenCodeWorld {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("OpenCodeWorld")
                .field("case", &self.case)
                .finish_non_exhaustive()
        }
    }

    #[given(expr = "a pinned V2 server exercising {word}")]
    fn server(w: &mut OpenCodeWorld, case: String) {
        w.case = case;
    }

    #[when("the real OpenCode Executor runs with two retries and a fallback model")]
    async fn execute(w: &mut OpenCodeWorld) {
        let scenario = match w.case.as_str() {
            "complete" | "checkpoint-refused" => Scenario::Complete,
            "disconnect" => Scenario::Recover,
            "incomplete" => Scenario::Incomplete,
            "log-unavailable" => Scenario::LogUnavailable,
            "malformed" => Scenario::Malformed,
            "uncertain-acceptance" => Scenario::UncertainAcceptance,
            "tool-effect" => Scenario::ProviderFailure {
                status: 429,
                side_effect: true,
            },
            "safe-rate-limit" => Scenario::ProviderFailure {
                status: 429,
                side_effect: false,
            },
            "safe-model-missing" => Scenario::ProviderFailure {
                status: 404,
                side_effect: false,
            },
            "plugin-disabled" => Scenario::PluginDisabled,
            "cancelled" => Scenario::Pending,
            _ => panic!("unknown contract case: {}", w.case),
        };
        let _lock = cruise::test_support::lock_process();
        let fixture = Fixture::new(scenario).await;
        let _guards = fixture.guards();
        // This fixture invokes Executor::new(Some("opencode"), &[]).run(PromptRun),
        // with no output callback. A retry barrier must work even without visible text.
        w.outcome = Some(if w.case == "checkpoint-refused" {
            let callback = |_: &str| -> cruise::error::Result<()> {
                Err(cruise::error::CruiseError::Other(
                    "checkpoint-refused".into(),
                ))
            };
            fixture
                .execute_with_checkpoint(Some(MODEL), 2, Some(&callback), None)
                .await
        } else if w.case == "cancelled" {
            let token = cruise::cancellation::CancellationToken::new();
            let run = fixture.execute_with_checkpoint(Some(MODEL), 2, None, Some(&token));
            tokio::pin!(run);
            tokio::select! {
                result = &mut run => panic!("execution ended before cancellation: {result:?}"),
                () = async {
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        while fixture.prompt_count() == 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        }
                    })
                    .await
                    .unwrap_or_else(|_| panic!("cancellation fixture never observed an active provider prompt"));
                    assert_eq!(fixture.prompt_count(), 1, "cancellation must target the first turn");
                } => token.cancel(),
            }
            run.await
        } else {
            fixture.execute(Some(MODEL), 2).await
        });
        // Observe cleanup before environment guards or fixture teardown can mask it.
        fixture.assert_stopped();
        w.fixture = Some(fixture);
    }

    #[then("the answer is returned exactly once without foreign or reasoning text")]
    fn answer(w: &mut OpenCodeWorld) {
        let outcome = w.outcome.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(outcome.result.output, ANSWER);
        assert_eq!(outcome.result.output.matches(ANSWER.trim()).count(), 1);
        assert!(outcome.session_id.is_some());
    }

    #[then("the execution fails rather than reporting acceptance or text as success")]
    fn failure(w: &mut OpenCodeWorld) {
        let error = w
            .outcome
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .to_string();
        // A generic setup/transport error must not accidentally satisfy a
        // scenario intended to exercise a later protocol or retry boundary.
        let expected: &[&str] = match w.case.as_str() {
            "incomplete" => &["execution outcome uncertain"],
            "log-unavailable" => &["durable reconciliation failed: OpenCode local API HTTP 503"],
            "malformed" => &["malformed SSE JSON"],
            "uncertain-acceptance" => &["prompt acceptance uncertain"],
            "tool-effect" => &["fixture provider failure"],
            "plugin-disabled" => &["setup timed out", "required native plugin"],
            "checkpoint-refused" => &["checkpoint-refused"],
            _ => panic!("no expected failure category for {}", w.case),
        };
        assert!(
            expected.iter().any(|marker| error.contains(marker)),
            "{} failed at the wrong boundary: {error}",
            w.case,
        );
    }

    #[then(expr = "exactly {int} provider prompts were submitted")]
    fn prompts(w: &mut OpenCodeWorld, expected: usize) {
        assert_eq!(
            w.fixture.as_ref().unwrap().launch_count(),
            expected.max(1),
            "{}: unexpected CLI attempts or hidden model fallback",
            w.case
        );
        assert_eq!(
            w.fixture.as_ref().unwrap().prompt_count(),
            expected,
            "{}: retry barrier or safe retry contract violated",
            w.case
        );
    }

    #[then("the owned CLI has been reaped before the Executor returns")]
    fn stopped(w: &mut OpenCodeWorld) {
        w.fixture.as_ref().unwrap().assert_stopped();
    }

    #[then("the active session was interrupted without converting cancellation into success")]
    fn interrupted(w: &mut OpenCodeWorld) {
        assert!(matches!(
            w.outcome.as_ref().unwrap(),
            Err(cruise::error::CruiseError::Interrupted)
        ));
        w.fixture.as_ref().unwrap().assert_interrupted();
    }

    pub async fn main() {
        OpenCodeWorld::cucumber()
            .max_concurrent_scenarios(1)
            // Domain tags such as @retry-barrier are not authorization to
            // rerun a failing specification. Any assertion failure must fail.
            .retry_options(|_, _, _, _| None)
            .fail_on_skipped()
            .run_and_exit(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/opencode.feature"
            ))
            .await;
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    contracts::main().await;
}

#[cfg(not(unix))]
fn main() {
    panic!(
        "opencode_bdd cannot run: POSIX CLI lifetime fixtures require Unix; unsupported platforms must not pass this contract suite"
    );
}
