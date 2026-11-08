//! `cruise config` -- show or update application-level configuration.
use crate::{cli::ConfigArgs, error::Result};

pub fn run(args: &ConfigArgs) -> Result<()> {
    let mut config = crate::app_config::AppConfig::load()?;
    if let Some(parallelism) = args.set_parallelism {
        config.run_all_parallelism = parallelism;
    }
    if let Some(language) = &args.set_language {
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
    if args.set_parallelism.is_some() || args.set_language.is_some() {
        config.save()?;
    }
    println!("run_all_parallelism = {}", config.run_all_parallelism);
    if args.set_parallelism.is_none() && args.set_language.is_none() {
        if let Some(model) = config.model.as_ref() {
            println!("model = {}", serde_json::to_string(model)?);
        }
        if let Some(model) = config.plan_model.as_ref() {
            println!("plan_model = {}", serde_json::to_string(model)?);
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
