//! Executable OpenCode V2 execution, common-effort, and no-route fallback contracts.
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
    use super::fixture::{
        ANSWER, Catalog, Fixture, MODEL, Scenario, effort_catalog, resume_fixture_with_model,
        selected_models, set_catalog,
    };
    use cruise::executor::PromptOutcome;
    use cucumber::{World as _, given, then, when};
    use serde_json::json;

    #[derive(Default, cucumber::World)]
    pub struct OpenCodeWorld {
        case: String,
        fixture: Option<Fixture>,
        outcome: Option<cruise::error::Result<PromptOutcome>>,
        baseline_prompts: usize,
        baseline_selections: usize,
        expected_model: serde_json::Value,
        retained_id: Option<String>,
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

    #[when(expr = "the Executor selects {word} on a {word} conversation")]
    async fn select(w: &mut OpenCodeWorld, case: String, conversation: String) {
        w.case = case.clone();
        let _lock = cruise::test_support::lock_process();
        let fixture = Fixture::new(Scenario::Complete).await;
        let _guards = fixture.guards();
        if conversation == "resumed" {
            let checkpoint = |_: &cruise::model_selection::SdkSessionId| Ok(());
            let first = fixture
                .execute_with_checkpoint(Some(MODEL), 0, Some(&checkpoint), None)
                .await
                .unwrap();
            w.retained_id = Some(first.session_id.unwrap().id);
        } else {
            assert_eq!(conversation, "new");
        }
        w.baseline_prompts = fixture.prompt_count();
        w.baseline_selections = selected_models(&fixture).len();
        let (raw, catalog, expected) = match case.as_str() {
            "canonical" => (
                Some("fixture/model:max"),
                effort_catalog(
                    "model",
                    &json!([{"id":"max"},{"id":"deep","settings":{"reasoningEffort":"max"}}]),
                ),
                json!({"providerID":"fixture","id":"model","variant":"max"}),
            ),
            "metadata" => (
                Some("fixture/model:free:high"),
                effort_catalog(
                    "model:free",
                    &json!([{"id":"deep","settings":{"reasoningEffort":"high"}}]),
                ),
                json!({"providerID":"fixture","id":"model:free","variant":"deep"}),
            ),
            "unsupported" => (
                Some("fixture/model:medium"),
                effort_catalog("model", &json!([{"id":"low"},{"id":"max"}])),
                json!(null),
            ),
            "ambiguous" => (
                Some("fixture/model:medium"),
                effort_catalog(
                    "model",
                    &json!([{"id":"a","settings":{"reasoningEffort":"medium"}},{"id":"b","settings":{"reasoningEffort":"medium"}}]),
                ),
                json!(null),
            ),
            "missing" => (
                Some("fixture/model:medium"),
                Catalog::Models(json!([])),
                json!(null),
            ),
            "protocol" => (
                Some("fixture/model:medium"),
                Catalog::InvalidJson,
                json!(null),
            ),
            "unauthorized" => (
                Some("fixture/model:medium"),
                Catalog::Http(axum::http::StatusCode::UNAUTHORIZED),
                json!(null),
            ),
            "unavailable" => (
                Some("fixture/model:medium"),
                Catalog::Http(axum::http::StatusCode::SERVICE_UNAVAILABLE),
                json!(null),
            ),
            "transport" => (
                Some("fixture/model:medium"),
                Catalog::Transport,
                json!(null),
            ),
            "native" => (
                Some(MODEL),
                Catalog::InvalidJson,
                json!({"providerID":"fixture","id":"team/model:2026","variant":"precise"}),
            ),
            "unknown" => (
                Some("fixture/model:free"),
                Catalog::InvalidJson,
                json!({"providerID":"fixture","id":"model:free"}),
            ),
            "plain" => (
                Some("fixture/model"),
                Catalog::InvalidJson,
                json!({"providerID":"fixture","id":"model"}),
            ),
            "unspecified" => (None, Catalog::InvalidJson, json!(null)),
            _ => panic!("unknown selection case: {case}"),
        };
        set_catalog(&fixture, catalog);
        w.expected_model = expected;
        w.outcome = Some(if let Some(id) = &w.retained_id {
            resume_fixture_with_model(&fixture, id, 2, raw.unwrap()).await
        } else {
            fixture.execute(raw, 2).await
        });
        fixture.assert_stopped();
        w.fixture = Some(fixture);
    }

    #[then(expr = "the catalog was requested {int} times")]
    fn catalogs(w: &mut OpenCodeWorld, count: usize) {
        assert_eq!(w.fixture.as_ref().unwrap().catalog_count(), count);
    }

    #[then("the selected wire model matches the catalog without altering the model ID")]
    fn wire_model(w: &mut OpenCodeWorld) {
        let fixture = w.fixture.as_ref().unwrap();
        assert_eq!(
            &selected_models(fixture)[w.baseline_selections..],
            std::slice::from_ref(&w.expected_model)
        );
        assert_eq!(fixture.prompt_count(), w.baseline_prompts + 1);
        if let Some(id) = &w.retained_id {
            assert_eq!(
                &w.outcome
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .session_id
                    .as_ref()
                    .unwrap()
                    .id,
                id
            );
        }
    }

    #[then("selection fails explicitly without a prompt or conversation model mutation")]
    fn selection_failure(w: &mut OpenCodeWorld) {
        let diagnostic = w
            .outcome
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .to_string()
            .to_ascii_lowercase();
        let markers: &[&str] = match w.case.as_str() {
            "unsupported" => &["unsupported", "fixture/model", "medium", "low", "max"],
            "ambiguous" => &["ambiguous", "fixture/model", "medium"],
            "missing" => &["catalog", "model"],
            "protocol" => &["protocol"],
            "unauthorized" => &["401"],
            "unavailable" => &["503"],
            "transport" => &["transport"],
            _ => panic!("unknown failure case"),
        };
        for marker in markers {
            assert!(
                diagnostic.contains(marker),
                "missing {marker}: {diagnostic}"
            );
        }
        let fixture = w.fixture.as_ref().unwrap();
        assert_eq!(fixture.prompt_count(), w.baseline_prompts);
        assert_eq!(selected_models(fixture).len(), w.baseline_selections);
        assert_eq!(
            fixture.launch_count(),
            if w.retained_id.is_some() { 2 } else { 1 }
        );
    }

    #[when(expr = "a no-route provider fails with {word} and retry budget {int}")]
    async fn no_route(w: &mut OpenCodeWorld, effect: String, budget: usize) {
        let _lock = cruise::test_support::lock_process();
        let fixture = Fixture::new(Scenario::NoRoute {
            side_effect: effect == "tool-effect",
        })
        .await;
        let _guards = fixture.guards();
        // Unique primary identity prevents another scenario's cooldown leaking in.
        let primary = format!("fixture/no-route-bdd-{effect}-{budget}");
        w.outcome = Some(fixture.execute(Some(&primary), budget).await);
        fixture.assert_stopped();
        w.fixture = Some(fixture);
    }

    #[then("no-route falls back once and retains the provider diagnostic")]
    fn switched(w: &mut OpenCodeWorld) {
        let fixture = w.fixture.as_ref().unwrap();
        let models = selected_models(fixture);
        assert_eq!(models.len(), 2);
        assert!(
            models[0]["id"]
                .as_str()
                .unwrap()
                .starts_with("no-route-bdd-")
        );
        assert_eq!(models[1], json!({"providerID":"fixture","id":"fallback"}));
        assert!(
            fixture
                .notices()
                .iter()
                .any(|n| n.contains("provider.no-route") && n.contains("Model unavailable"))
        );
    }

    #[then("no-route fails with its original diagnostic and never switches")]
    fn not_switched(w: &mut OpenCodeWorld) {
        let diagnostic = w
            .outcome
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .to_string();
        assert!(
            diagnostic.contains("provider.no-route") && diagnostic.contains("Model unavailable"),
            "{diagnostic}"
        );
        assert_eq!(selected_models(w.fixture.as_ref().unwrap()).len(), 1);
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
            let callback =
                |session: &cruise::model_selection::SdkSessionId| -> cruise::error::Result<()> {
                    assert_eq!(session.sdk, cruise::model_selection::Sdk::OpenCode);
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
        } else if w.case.starts_with("safe-") {
            // A rate limit now cools the primary down process-wide; a unique
            // primary identity keeps it from leaking into the next scenario.
            let primary = format!("fixture/{}", w.case);
            fixture.execute(Some(&primary), 2).await
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
