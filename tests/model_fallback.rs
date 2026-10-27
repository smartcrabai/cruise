use std::path::Path;

use cruise::config::WorkflowConfig;
use cruise::retry::active_policy;
use cruise::test_support::{EnvGuard, ProcessLock, lock_process};
use cruise::workflow;
use cruise::workflow_call::resolve_workflow_calls_with_defaults;
use cruise::workflow_defaults::WorkflowDefaults;
use tempfile::TempDir;

struct ResolutionEnv {
    temp: TempDir,
    _guards: Vec<EnvGuard>,
    _lock: ProcessLock,
}

impl ResolutionEnv {
    fn new() -> Self {
        let lock = lock_process();
        let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let config_dir = temp.path().join("config");
        let guards = vec![
            EnvGuard::set("XDG_CONFIG_HOME", config_dir.as_os_str()),
            EnvGuard::remove("CRUISE_MODEL"),
            EnvGuard::remove("CRUISE_PLAN_MODEL"),
            EnvGuard::remove("CRUISE_SDK"),
        ];
        Self {
            temp,
            _guards: guards,
            _lock: lock,
        }
    }

    fn write_app_config(&self, content: &str) {
        let path = self.temp.path().join("config/cruise/config.json");
        std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .unwrap_or_else(|error| panic!("create app config directory: {error}"));
        std::fs::write(path, content).unwrap_or_else(|error| panic!("write app config: {error}"));
    }
}

#[test]
fn scheduled_candidates_survive_app_defaults_resolution_compile_and_after_pr() {
    for extension in ["json", "yaml"] {
        for from_defaults in [false, true] {
            let env = ResolutionEnv::new();
            let model = serde_json::json!([
                {"model": "exec/main:high", "disabled_during": ["Mon-Fri 01:00-04:00", "06:00-10:00 +09:00"]},
                {"sdk": "claude", "model": "exec/tail", "disabled_during": ["00:00-24:00"]}
            ]);
            let plan_model = serde_json::json!([
                {"sdk": "claude", "model": "plan/main", "disabled_during": ["fri 22:00-02:00 -05:30"]},
                {"model": "plan/tail", "disabled_during": ["sat,sun 00:00-24:00"]}
            ]);
            let input = serde_json::json!({"model": model, "planModel": plan_model});
            let app_dir = env.temp.path().join("config/cruise");
            std::fs::create_dir_all(&app_dir)
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            let content = if extension == "yaml" {
                serde_yaml::to_string(&input)
                    .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"))
            } else {
                input.to_string()
            };
            std::fs::write(app_dir.join(format!("config.{extension}")), content)
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            let project = env.temp.path().join("project");
            std::fs::create_dir_all(project.join(".cruise"))
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            if from_defaults {
                // Defaults must win over app configuration without dropping raw schedules.
                std::fs::write(
                    project.join(".cruise/default.yaml"),
                    format!("model: {model}\nplan_model: {plan_model}\n"),
                )
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
                let app = serde_json::json!({"model": "app-loser", "plan_model": "app-plan-loser"});
                std::fs::write(app_dir.join(format!("config.{extension}")), app.to_string())
                    .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            }
            let defaults = WorkflowDefaults::load(Some(&project))
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            let config = resolve_workflow_calls_with_defaults(
                WorkflowConfig::from_yaml("steps:\n  main:\n    prompt: execute\nafter_pr:\n  review:\n    prompt: review\n").unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}")),
                &project,
                &defaults,
            ).unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            cruise::config::validate_config(&config)
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            assert_eq!(model_value(&config.model), model);
            assert_eq!(model_value(&config.plan_model), plan_model);
            let compiled = workflow::compile(config)
                .unwrap_or_else(|error| panic!("schedule fixture operation failed: {error}"));
            assert_eq!(model_value(&compiled.model), model);
            assert_eq!(model_value(&compiled.plan_model), plan_model);
            let after_pr = compiled.to_after_pr_compiled();
            assert_eq!(model_value(&after_pr.model), model);
            assert_eq!(model_value(&after_pr.plan_model), plan_model);
        }
    }
}

fn model_value(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|error| panic!("serialize model selection: {error}"))
}

#[test]
fn empty_legacy_candidate_arrays_remain_accepted_in_workflow_and_app_formats() {
    let _env = ResolutionEnv::new();
    let config = resolve("model: []\nplan_model: []\nsteps:\n  main:\n    prompt: execute\n");
    cruise::config::validate_config(&config)
        .unwrap_or_else(|error| panic!("legacy empty arrays must remain valid: {error}"));
    assert_eq!(model_value(&config.model), serde_json::json!([]));
    assert_eq!(model_value(&config.plan_model), serde_json::json!([]));
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    for extension in ["json", "yaml"] {
        let path = temp.path().join(format!("config.{extension}"));
        std::fs::write(&path, r#"{"model":[],"plan_model":[]}"#)
            .unwrap_or_else(|error| panic!("write empty {extension} arrays: {error}"));
        let app = cruise::app_config::AppConfig::load_from(&path)
            .unwrap_or_else(|error| panic!("load legacy empty {extension} arrays: {error}"));
        app.save_to(&path)
            .unwrap_or_else(|error| panic!("save legacy empty {extension} arrays: {error}"));
        let loaded = cruise::app_config::AppConfig::load_from(&path)
            .unwrap_or_else(|error| panic!("reload empty {extension} arrays: {error}"));
        assert_eq!(model_value(&loaded.model), serde_json::json!([]));
        assert_eq!(model_value(&loaded.plan_model), serde_json::json!([]));
    }
}

#[test]
fn compile_and_after_pr_keep_distinct_exec_and_plan_candidate_lists() {
    let _env = ResolutionEnv::new();
    let model = serde_json::json!([
        "shared/model:medium",
        {"sdk": "claude", "model": "exec/fallback:high"},
        "openrouter/z-ai/glm-5.3-flash:high"
    ]);
    let plan_model = serde_json::json!([
        "shared/model:medium",
        {"sdk": "claude", "model": "plan/fallback:high"}
    ]);
    let yaml = format!(
        "model: {model}\nplan_model: {plan_model}\nsteps:\n  main:\n    prompt: execute\nafter_pr:\n  review:\n    prompt: review\n"
    );
    let compiled = workflow::compile(resolve(&yaml))
        .unwrap_or_else(|error| panic!("compile mixed candidate lists: {error}"));
    assert_eq!(model_value(&compiled.model), model);
    assert_eq!(model_value(&compiled.plan_model), plan_model);

    let after_pr = compiled.to_after_pr_compiled();
    assert_eq!(model_value(&after_pr.model), model);
    assert_eq!(model_value(&after_pr.plan_model), plan_model);
}

#[test]
fn app_config_json_and_yaml_candidates_survive_runtime_resolution_and_compile() {
    for extension in ["json", "yaml"] {
        let env = ResolutionEnv::new();
        let model = serde_json::json!([
            "openai/gpt-6.1-sol:medium",
            {"sdk": "claude", "model": "claude-sonnet-5-5:medium"},
            "openrouter/z-ai/glm-5.3-flash:high"
        ]);
        let plan_model = serde_json::json!([
            {"sdk": "claude", "model": "planning-primary:high"},
            "planning/fallback:medium"
        ]);
        let path = env
            .temp
            .path()
            .join(format!("config/cruise/config.{extension}"));
        std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .unwrap_or_else(|error| panic!("create config directory: {error}"));
        let input = serde_json::json!({"model": model, "plan_model": plan_model});
        // JSON is valid YAML too. Using the same document exercises both file
        // loaders without making their candidate semantics diverge.
        std::fs::write(path, input.to_string())
            .unwrap_or_else(|error| panic!("write {extension}: {error}"));
        let resolved = resolve("steps:\n  main:\n    prompt: execute\n");
        let compiled = workflow::compile(resolved)
            .unwrap_or_else(|error| panic!("compile {extension} candidates: {error}"));
        assert_eq!(model_value(&compiled.model), model, "{extension}");
        assert_eq!(model_value(&compiled.plan_model), plan_model, "{extension}");
    }
}

#[test]
fn mixed_candidates_follow_workflow_then_defaults_then_app_precedence() {
    let env = ResolutionEnv::new();
    let app_model = serde_json::json!([{"sdk": "claude", "model": "app-model"}]);
    let app_plan = serde_json::json!([{"sdk": "claude", "model": "app-plan"}]);
    env.write_app_config(
        &serde_json::json!({"model": app_model, "plan_model": app_plan}).to_string(),
    );
    let defaults_dir = env.temp.path().join("project/.cruise");
    std::fs::create_dir_all(&defaults_dir)
        .unwrap_or_else(|error| panic!("create defaults directory: {error}"));
    let default_model =
        serde_json::json!(["default-primary", {"sdk": "claude", "model": "default-tail"}]);
    std::fs::write(
        defaults_dir.join("default.yaml"),
        format!("model: {default_model}\n"),
    )
    .unwrap_or_else(|error| panic!("write defaults: {error}"));
    let project = env.temp.path().join("project");
    let defaults = WorkflowDefaults::load(Some(&project))
        .unwrap_or_else(|error| panic!("load mixed defaults: {error}"));

    let inherited = resolve_workflow_calls_with_defaults(
        WorkflowConfig::from_yaml("steps:\n  main:\n    prompt: execute\n")
            .unwrap_or_else(|error| panic!("parse workflow: {error}")),
        &project,
        &defaults,
    )
    .unwrap_or_else(|error| panic!("resolve defaults and app: {error}"));
    assert_eq!(model_value(&inherited.model), default_model);
    assert_eq!(model_value(&inherited.plan_model), app_plan);

    let workflow_model =
        serde_json::json!([{"sdk": "claude", "model": "workflow-primary"}, "workflow-tail"]);
    let explicit = resolve_workflow_calls_with_defaults(
        WorkflowConfig::from_yaml(&format!(
            "model: {workflow_model}\nsteps:\n  main:\n    prompt: execute\n"
        ))
        .unwrap_or_else(|error| panic!("parse explicit workflow: {error}")),
        &project,
        &defaults,
    )
    .unwrap_or_else(|error| panic!("resolve explicit workflow: {error}"));
    assert_eq!(model_value(&explicit.model), workflow_model);
    assert_eq!(model_value(&explicit.plan_model), app_plan);
}

#[test]
fn environment_model_overrides_replace_whole_mixed_lists_without_stale_fallbacks() {
    let _env = ResolutionEnv::new();
    let _model = EnvGuard::set("CRUISE_MODEL", "env/execution:high");
    let _plan = EnvGuard::set("CRUISE_PLAN_MODEL", "env/planning:medium");
    let resolved = resolve(
        "model: [old/primary, {sdk: claude, model: old/fallback}]\nplan_model: [old/plan, {sdk: claude, model: old/plan-fallback}]\nsteps:\n  main:\n    prompt: execute\n",
    );
    assert_eq!(
        model_value(&resolved.model),
        serde_json::json!("env/execution:high")
    );
    assert_eq!(
        model_value(&resolved.plan_model),
        serde_json::json!("env/planning:medium")
    );
    let compiled = workflow::compile(resolved)
        .unwrap_or_else(|error| panic!("compile environment overrides: {error}"));
    assert_eq!(
        model_value(&compiled.model),
        serde_json::json!("env/execution:high")
    );
    assert_eq!(
        model_value(&compiled.plan_model),
        serde_json::json!("env/planning:medium")
    );
}

fn resolve(yaml: &str) -> WorkflowConfig {
    let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
    resolve_workflow_calls_with_defaults(config, ".", &WorkflowDefaults::default())
        .unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn resolving_array_models_preserves_candidates_and_enables_fallback() {
    let _env = ResolutionEnv::new();
    let config = resolve(
        r"
 sdk: jcode
 model:
   - anthropic/primary
   - openai/fallback
   - google/last-resort
 plan_model:
   - anthropic/planning
   - openai/planning-fallback
 steps:
   s1:
     prompt: hi
 ",
    );

    let compiled = workflow::compile(config.clone()).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(
        model_value(&compiled.model),
        serde_json::json!(["anthropic/primary", "openai/fallback", "google/last-resort"])
    );
    assert_eq!(
        model_value(&compiled.plan_model),
        serde_json::json!(["anthropic/planning", "openai/planning-fallback"])
    );

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("array models must create a retry policy"));
    assert!(
        retry.model_fallback,
        "array models must enable model fallback"
    );
    assert!(
        active_policy().is_some(),
        "resolved policy must be published"
    );
}

#[test]
fn explicit_retry_chain_takes_precedence_over_the_array_tail() {
    let _env = ResolutionEnv::new();
    let config = resolve(
        r"
 sdk: jcode
 model:
   - anthropic/primary
   - openai/array-fallback
 retry:
   base_delay_ms: 250
   fallback_chains:
     anthropic/primary:
       - google/explicit-fallback
 steps:
   s1:
     prompt: hi
 ",
    );

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("retry block must be preserved"));
    assert_eq!(retry.base_delay_ms, 250);
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/primary")
            .map(Vec::as_slice),
        Some(["google/explicit-fallback".to_string()].as_slice())
    );
}

#[test]
fn app_config_model_lists_build_primary_and_fallbacks_for_exec_and_planning() {
    let env = ResolutionEnv::new();
    env.write_app_config(
        r#"{"model":["anthropic/app-primary","openai/app-fallback","google/app-last"],"plan_model":["google/plan-primary","openai/plan-fallback"]}"#,
    );
    let config = resolve(
        r"
 sdk: jcode
 steps:
   s1:
     prompt: hi
 ",
    );

    let expected_model = serde_json::json!([
        "anthropic/app-primary",
        "openai/app-fallback",
        "google/app-last"
    ]);
    let expected_plan = serde_json::json!(["google/plan-primary", "openai/plan-fallback"]);
    assert_eq!(model_value(&config.model), expected_model);
    assert_eq!(model_value(&config.plan_model), expected_plan);
    let compiled = workflow::compile(config.clone()).unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(model_value(&compiled.model), expected_model);
    assert_eq!(model_value(&compiled.plan_model), expected_plan);

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("app model lists must create a retry policy"));
    assert!(retry.model_fallback);
}

#[test]
fn explicit_retry_chain_overrides_app_config_model_list_fallback() {
    let env = ResolutionEnv::new();
    env.write_app_config(r#"{"model":["anthropic/app-primary","openai/app-fallback"]}"#);
    let config = resolve(
        r"
 sdk: jcode
 retry:
   fallback_chains:
     anthropic/app-primary:
       - google/explicit-fallback
 steps:
   s1:
     prompt: hi
 ",
    );

    let retry = config
        .retry
        .as_ref()
        .unwrap_or_else(|| panic!("explicit retry block must be preserved"));
    assert_eq!(
        retry
            .fallback_chains
            .get("anthropic/app-primary")
            .map(Vec::as_slice),
        Some(["google/explicit-fallback".to_string()].as_slice())
    );
}
