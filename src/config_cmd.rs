//! `cruise config` -- show or update application-level configuration.
use crate::{cli::ConfigArgs, error::Result};

/// Show or update application configuration.
///
/// # Errors
/// Returns an error if configuration cannot be loaded, validated, or saved.
pub fn run(args: &ConfigArgs) -> Result<()> {
    let mut config = crate::app_config::AppConfig::load()?;
    let updating = args.parallelism.is_some()
        || args.auto_requeue_max_attempts.is_some()
        || args.language.is_some();
    if let Some(parallelism) = args.parallelism {
        config.run_all_parallelism = parallelism;
    }
    if let Some(max) = args.auto_requeue_max_attempts {
        config.run_all_auto_requeue_max_attempts = max;
    }
    if let Some(language) = &args.language {
        config.ui_language = if language.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(
                crate::i18n::Locale::from_tag(language)
                    .ok_or_else(|| {
                        crate::error::CruiseError::Other(format!("invalid UI language: {language}"))
                    })?
                    .tag()
                    .to_owned(),
            )
        };
    }
    if updating {
        config.save()?;
        if let Some(parallelism) = args.parallelism {
            println!("run_all_parallelism = {parallelism}");
        }
        if let Some(max) = args.auto_requeue_max_attempts {
            println!("run_all_auto_requeue_max_attempts = {max}");
        }
    } else {
        config = config.with_env_overrides()?;
        let source = if std::env::var("CRUISE_RUN_ALL_PARALLELISM")
            .is_ok_and(|value| !value.trim().is_empty())
        {
            " (from CRUISE_RUN_ALL_PARALLELISM)"
        } else {
            ""
        };
        println!(
            "run_all_parallelism = {}{source}",
            config.run_all_parallelism
        );
        println!(
            "run_all_auto_requeue_max_attempts = {}",
            config.run_all_auto_requeue_max_attempts
        );
        println!("prevent_sleep = {}", config.prevent_sleep);
        if let Some(model) = config.model.as_ref() {
            println!("model = {}", serde_json::to_string(model)?);
        }
        if let Some(model) = config.plan_model.as_ref() {
            println!("plan_model = {}", serde_json::to_string(model)?);
        }
        if let Some(languages) = config
            .languages
            .as_ref()
            .filter(|languages| languages.pr.is_some() || languages.plan.is_some())
        {
            println!("languages = {}", serde_json::to_string(languages)?);
        }
        if let Some(template) = config.commit_message_template.as_ref() {
            println!(
                "commit_message_template = {}",
                serde_json::to_string(template)?
            );
        }
        if let Some(template) = config.pr_body_template.as_ref() {
            println!("pr_body_template = {}", serde_json::to_string(template)?);
        }
        if !config.notifications.is_default() {
            println!(
                "notifications = {}",
                serde_json::to_string(&config.notifications)?
            );
        }
        if let Some((timestamp, error)) = crate::notification_webhook::last_error() {
            println!("notification_webhook_error = {error} at {timestamp}");
        }
    }
    let resolved = crate::i18n::resolve::resolve_current(config.ui_language.as_deref());
    println!(
        "ui_language = {} (source: {})",
        resolved.locale.tag(),
        resolved.source.as_str()
    );
    Ok(())
}
