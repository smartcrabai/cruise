use crate::cli::ExecArgs;
use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::resolver::{ConfigSource, load_config_from_source, resolve_config};
use crate::session::SessionPhase;
use crate::session_config::SessionConfigRef;

pub(crate) struct ExecRequest {
    pub input: String,
    pub max_retries: Option<usize>,
    pub rate_limit_retries: usize,
    pub dry_run: bool,
    pub flags: Vec<(String, bool)>,
}

pub async fn run(args: ExecArgs) -> Result<()> {
    if args.instant {
        let yaml = crate::instant_exec::build_yaml(&args)?;
        let source = ConfigSource::Inline;
        let config = load_config_from_source(&yaml, &source)?;
        return run_resolved(
            &config,
            &yaml,
            &source,
            ExecRequest {
                input: args.input.clone().unwrap_or_default(),
                max_retries: args.max_retries,
                rate_limit_retries: args.rate_limit_retries,
                dry_run: args.dry_run,
                flags: Vec::new(),
            },
        )
        .await;
    }
    let (yaml, source) = resolve_config(args.config.as_deref())?;
    let config = load_config_from_source(&yaml, &source)?;
    run_resolved(
        &config,
        &yaml,
        &source,
        ExecRequest {
            input: args.input.unwrap_or_default(),
            max_retries: args.max_retries,
            rate_limit_retries: args.rate_limit_retries,
            dry_run: args.dry_run,
            flags: args.flags,
        },
    )
    .await
}

pub(crate) async fn run_resolved(
    config: &WorkflowConfig,
    yaml: &str,
    source: &ConfigSource,
    req: ExecRequest,
) -> Result<()> {
    let effective_max_retries =
        crate::config::resolve_effective_max_retries(req.max_retries, config);
    crate::config::validate_group_retry_budget(config, effective_max_retries)?;

    let compiled = crate::workflow::compile(config.clone())?;
    let graph = crate::graph::build_graph(&compiled, effective_max_retries)?;
    let flag_values = crate::flags::from_cli(&req.flags)?;
    crate::flags::validate_names(config, &flag_values)?;
    if req.dry_run {
        crate::graph::validation::validate_workflow(
            &compiled,
            &graph,
            &graph.start,
            &crate::flags::preflight_skips(&compiled, &flag_values, &[]),
        )?;
        crate::engine::print_dry_run(config, None);
        return Ok(());
    }

    let _herdr = crate::herdr::start();
    let flag_values = if req.flags.is_empty() {
        crate::run_cmd::select_flag_values(config, &crate::flags::FlagValues::new())?
            .ok_or_else(|| CruiseError::Other("workflow flag selection cancelled".into()))?
    } else {
        flag_values
    };
    let context = crate::request_context::RequestContext::capture()?;
    let request = crate::application::ExecStartRequest {
        config_yaml: yaml.to_owned(),
        config_source: SessionConfigRef::from_source(source, None)?,
        input: req.input,
        flags: flag_values,
        max_retries: req.max_retries,
        rate_limit_retries: req.rate_limit_retries,
        context,
    };

    let client = crate::daemon::client::DaemonClient::connect().await?;
    let stream = client
        .events(crate::daemon::client::EventFilter::default())
        .await?;
    let session_id = client.start_exec(&request).await?;
    let mut handler = crate::cli_events::TerminalEvents::new(&client, &session_id);
    let result = crate::cli_events::drive(&client, &session_id, stream, &mut handler)
        .await
        .and_then(exec_exit_result);
    crate::run_cmd::finish_cli_run(result)
}

fn exec_exit_result(finished: crate::cli_events::Finished) -> Result<()> {
    if finished.cancelled_by_user {
        return Err(CruiseError::Interrupted);
    }
    match finished.state.phase {
        SessionPhase::Completed => Ok(()),
        SessionPhase::Failed(message) => Err(CruiseError::Other(message)),
        SessionPhase::PrFailed(message) => Err(CruiseError::PrPublicationFailed(message)),
        SessionPhase::Suspended => Err(CruiseError::Interrupted),
        phase => Err(CruiseError::Other(format!(
            "exec session ended in '{}' phase",
            phase.label()
        ))),
    }
}
