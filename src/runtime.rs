use crate::{
    app_config, backend, ci_followup, clean_cmd, cli, config_cmd, daemon, diff_cmd, docs_cmd,
    draft_cmd, error, exec_cmd, host_sync, i18n, ledger_snapshot, list_cmd, manager_cmd, plan_cmd,
    platform, provider_install, run_cmd, ssh_cmd, telemetry, tell_cmd, tool_policy, update_cmd,
    usage_cmd, workflow_cmd, workflow_doctor, workflow_generate, workflow_make, worktree_pr,
};

// Hooks run once per tool call, without constructing a runtime or telemetry.
#[doc(hidden)]
pub fn entry() {
    let cli = cli::parse_cli();
    if let Some(cli::Commands::Docs(command)) = &cli.command {
        std::process::exit(docs_cmd::run(command));
    }
    if let Some(cli::Commands::Workflow(cli::WorkflowCommand::Doctor(args))) = &cli.command {
        std::process::exit(workflow_doctor::run(args));
    }
    if matches!(
        &cli.command,
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::JcodePreTool,
        }))
    ) {
        std::process::exit(tool_policy::run_jcode_pre_tool_gate());
    }
    #[cfg(all(target_os = "linux", feature = "jcode-bubblewrap"))]
    if let Some(cli::Commands::Internal(cli::InternalArgs {
        command:
            cli::InternalCommand::JcodeRelay {
                socket,
                ready,
                port,
            },
    })) = &cli.command
    {
        match backend::jcode::relay(socket, ready, *port) {
            Ok(()) => return,
            Err(error) => {
                eprintln!("cruise: jcode namespace relay failed: {error}");
                std::process::exit(1);
            }
        }
    }
    // Removed bridge registrations must fail before runtime initialization.
    if matches!(&cli.command, Some(cli::Commands::McpBridge(_))) {
        let mcp_json = backend::jcode::resolve_source_home(None).join("mcp.json");
        eprintln!("cruise: {}", cli::removed_mcp_bridge_message(&mcp_json));
        std::process::exit(1);
    }
    runtime_main(cli);
}

// Multi-threaded runtime: parallel `run --all` workers execute blocking
// terminal prompts inline in their tasks; on a single-threaded runtime any
// open menu would freeze every other concurrently running session.
#[tokio::main]
async fn runtime_main(cli: cli::Cli) {
    let config = match app_config::AppConfig::load() {
        Ok(config) => Some(config),
        Err(error) => {
            eprintln!("warning: ignoring app configuration for UI language: {error}");
            None
        }
    };
    let resolved =
        i18n::resolve::resolve_current(config.as_ref().and_then(|c| c.ui_language.as_deref()));
    for warning in &resolved.warnings {
        eprintln!("{warning}");
    }
    i18n::init_process_locale(resolved.locale);
    let telemetry = match telemetry::init() {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!(
                "{}: {error}",
                i18n::t(i18n::process_locale(), "webui.session.error")
            );
            std::process::exit(1);
        }
    };
    let result = Box::pin(run(cli)).await;
    telemetry.shutdown();
    if let Err(e) = result {
        eprintln!(
            "{}: {}",
            i18n::t(i18n::process_locale(), "webui.session.error"),
            e.detailed_message()
        );
        std::process::exit(1);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "The top-level command router must cover each CLI command variant."
)]
async fn run(cli: cli::Cli) -> error::Result<()> {
    let cli::Cli {
        plan,
        command,
        input,
        no_force_exec,
        skip_planning,
        repo,
        images,
        flags,
    } = cli;
    match command {
        Some(cli::Commands::Plan(args)) => Box::pin(plan_cmd::run(args)).await,
        Some(cli::Commands::Draft(args)) => draft_cmd::run(args).await,
        Some(cli::Commands::Run(args)) => run_cmd::run(args).await,
        Some(cli::Commands::Tell(args)) => tell_cmd::run(args).await,
        Some(cli::Commands::List(args)) => list_cmd::run(args).await,
        Some(cli::Commands::Manager(args)) => manager_cmd::run(args).await,
        Some(cli::Commands::Diff(args)) => diff_cmd::run(&args).await,
        Some(cli::Commands::Clean(args)) => clean_cmd::run(args).await,
        Some(cli::Commands::Usage(args)) => usage_cmd::run(&args),
        Some(cli::Commands::Config(args)) => config_cmd::run(&args),
        Some(cli::Commands::Exec(args)) => exec_cmd::run(args).await,
        Some(cli::Commands::Ssh(args)) => ssh_cmd::run(&args),
        Some(cli::Commands::Daemon(args)) => match args.command {
            cli::DaemonCommand::Start(args) => {
                daemon::lifecycle::start(&daemon_serve_options(args))
            }
            cli::DaemonCommand::Stop(args) => {
                daemon::lifecycle::stop(daemon::StopOptions { force: args.force })
            }
            cli::DaemonCommand::Restart(args) => {
                daemon::lifecycle::restart(daemon_serve_options(args))
            }
            cli::DaemonCommand::Status(args) => {
                daemon::lifecycle::status(daemon::StatusOptions { json: args.json })
            }
            cli::DaemonCommand::Serve(args) => {
                daemon::server::run_foreground(daemon_serve_options(args)).await
            }
        },
        Some(cli::Commands::Webui(args)) => {
            let client = daemon::client::DaemonClient::connect().await?;
            let login_url = client.webui_login_url().await?;
            println!("{login_url}");
            if args.open
                && let Err(error) = platform::open_login_redirect(&login_url)
            {
                eprintln!("warning: could not open browser: {error}");
            }
            Ok(())
        }
        Some(cli::Commands::Update(args)) => update_cmd::run(&args),
        Some(cli::Commands::Install(args)) => {
            provider_install::run(args.provider, args.status, args.force)
        }
        Some(cli::Commands::Workflow(command)) => run_workflow(command).await,
        Some(cli::Commands::Docs(_)) => {
            unreachable!("docs command exits before runtime initialization")
        }
        Some(cli::Commands::Internal(cli::InternalArgs {
            command:
                cli::InternalCommand::DaemonServe {
                    instance_id,
                    host,
                    port,
                    port_implicit,
                    webui_dir,
                },
        })) => daemon::server::run_daemon(instance_id, host, port, port_implicit, webui_dir).await,
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::SyncBase { pr_number, url },
        })) => {
            let plan = std::env::var_os("CRUISE_SYNC_PLAN").map(std::path::PathBuf::from);
            let cwd = std::env::current_dir()?;
            let result = if let Some(plan) = &plan {
                let base = host_sync::clear_sync_result(plan, &cwd, &pr_number, url.as_deref())?;
                worktree_pr::sync_validated_base(&cwd, &base)
            } else {
                worktree_pr::sync_base(&pr_number, url.as_deref())
            };
            if let Err((failure, _)) = &result {
                eprintln!("CRUISE_SYNC_BASE_DIAGNOSTIC_V1 {}", failure.marker_value());
            }
            if let Some(plan) = &plan {
                let outcome = result.as_ref().copied().map_err(|(kind, _)| *kind);
                host_sync::record_sync_result(plan, &cwd, &pr_number, url.as_deref(), outcome)?;
            }
            match result {
                Ok(()) => Ok(()),
                Err((failure, error)) => {
                    if plan.is_some() && failure == worktree_pr::SyncBaseFailure::MergeConflict {
                        eprintln!("{error}");
                        std::process::exit(75);
                    }
                    Err(error)
                }
            }
        }
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::SyncReviewHead,
        })) => worktree_pr::sync_review_head(&std::env::current_dir()?),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::TddLedgerSnapshot,
        })) => ledger_snapshot::refresh_after_apply(
            &std::env::current_dir()?,
            &path_from_env("CRUISE_TDD_LEDGER_PLAN")?,
        ),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::FinishConflictCommit,
        })) => host_sync::finish_conflict_commit(
            &path_from_env("CRUISE_SYNC_PLAN")?,
            &std::env::current_dir()?,
        ),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::PushAfterSync,
        })) => host_sync::push_after_sync(
            &path_from_env("CRUISE_SYNC_PLAN")?,
            &std::env::current_dir()?,
        ),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::ClearCiFollowup,
        })) => ci_followup::clear(
            &std::env::current_dir()?,
            &path_from_env("CRUISE_AFTER_PR_PLAN")?,
        ),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::CaptureCiFollowup,
        })) => {
            use std::io::Read as _;
            const MAX_DIAGNOSTIC: usize = 32 * 1024;
            let mut diagnostic = String::new();
            std::io::stdin()
                .take((MAX_DIAGNOSTIC + 1) as u64)
                .read_to_string(&mut diagnostic)?;
            if diagnostic.len() > MAX_DIAGNOSTIC {
                return Err(error::CruiseError::Other(
                    "CI diagnostic exceeds the size limit".into(),
                ));
            }
            ci_followup::evidence(
                &std::env::current_dir()?,
                &path_from_env("CRUISE_AFTER_PR_PLAN")?,
                &text_from_env("CRUISE_AFTER_PR_NUMBER")?,
                &text_from_env("CRUISE_AFTER_PR_URL")?,
                &diagnostic,
            )
        }
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::FinishCiFollowup,
        })) => ci_followup::finish(
            &std::env::current_dir()?,
            &path_from_env("CRUISE_AFTER_PR_PLAN")?,
            &text_from_env("CRUISE_AFTER_PR_NUMBER")?,
            &text_from_env("CRUISE_AFTER_PR_URL")?,
            run_approved_ci_verifier,
        ),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::RerunUnacquiredCi,
        })) => ci_followup::rerun_unacquired(
            &std::env::current_dir()?,
            &path_from_env("CRUISE_AFTER_PR_PLAN")?,
            &text_from_env("CRUISE_AFTER_PR_NUMBER")?,
            &text_from_env("CRUISE_AFTER_PR_URL")?,
            app_config::AppConfig::load_runtime()?.ci_unacquired_rerun_max_attempts,
        ),
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::JcodePreTool,
        })) => unreachable!("jcode pre-tool gate exits before runtime initialization"),
        #[cfg(all(target_os = "linux", feature = "jcode-bubblewrap"))]
        Some(cli::Commands::Internal(cli::InternalArgs {
            command: cli::InternalCommand::JcodeRelay { .. },
        })) => unreachable!("jcode relay exits before runtime initialization"),
        Some(cli::Commands::McpBridge(_)) => {
            unreachable!("removed mcp-bridge exits before runtime initialization")
        }
        None if plan.is_some() => {
            Box::pin(plan_cmd::launch_background_plan(
                &plan.unwrap_or_default(),
                skip_planning,
                repo.as_deref(),
                &images,
                no_force_exec,
                &flags,
            ))
            .await
        }
        None if input.is_none()
            && !skip_planning
            && !no_force_exec
            && repo.is_none()
            && images.is_empty()
            && flags.is_empty() =>
        {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin())
                || !std::io::IsTerminal::is_terminal(&std::io::stdout())
            {
                return Err(error::CruiseError::Other(
                    "cruise requires an interactive TTY".to_string(),
                ));
            }
            list_cmd::run(cli::ListArgs { json: false }).await
        }
        None => {
            // Backward compat: positional input and planning options use `plan`.
            let plan_args = cli::PlanArgs {
                input,
                pr: None,
                issue: None,
                config: None,
                dry_run: false,
                no_force_exec,
                skip_planning,
                grill: false,
                no_interactive_planning: false,
                repo,
                rate_limit_retries: cli::DEFAULT_RATE_LIMIT_RETRIES,
                images,
                flags,
            };
            Box::pin(plan_cmd::run(plan_args)).await
        }
    }
}

fn daemon_serve_options(args: cli::ServeArgs) -> daemon::ServeOptions {
    daemon::ServeOptions {
        host: args.host,
        port: args.port,
        open: args.open,
        port_explicit: None,
        webui_dir: args.webui_dir,
    }
}

async fn run_workflow(command: cli::WorkflowCommand) -> error::Result<()> {
    match command {
        cli::WorkflowCommand::List => workflow_cmd::list(),
        cli::WorkflowCommand::Eject(args) => {
            workflow_cmd::eject(&args.name, args.to == cli::EjectDestination::Project)
        }
        cli::WorkflowCommand::Generate(args) => workflow_generate::run(args).await,
        cli::WorkflowCommand::Make(args) => workflow_make::run(args).await,
        cli::WorkflowCommand::Doctor(_) => {
            unreachable!("workflow doctor exits before runtime initialization")
        }
        cli::WorkflowCommand::Add(args) => workflow_cmd::add(&args.spec, args.name, args.yes),
        cli::WorkflowCommand::Update(args) => workflow_cmd::update(&args.name, args.yes),
        cli::WorkflowCommand::Remove(args) => workflow_cmd::remove(&args.name),
    }
}

fn path_from_env(name: &str) -> error::Result<std::path::PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| error::CruiseError::Other(format!("missing {name}")))
}

fn text_from_env(name: &str) -> error::Result<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error::CruiseError::Other(format!("missing or invalid {name}")))
}

/// Verify a staged CI repair with the daemon's trusted script inside
/// Bubblewrap. Without Bubblewrap or a configured script, warn and allow the
/// commit: CI verifies the pushed head. A verifier that runs and fails blocks it.
#[cfg_attr(
    not(all(target_os = "linux", feature = "jcode-bubblewrap")),
    expect(
        clippy::unnecessary_wraps,
        reason = "the Linux Bubblewrap verifier is fallible; ci_followup::finish takes one signature"
    )
)]
fn run_approved_ci_verifier(candidate: &std::path::Path) -> error::Result<()> {
    #[cfg(all(target_os = "linux", feature = "jcode-bubblewrap"))]
    if backend::jcode::bubblewrap_available()
        && let Some(script) =
            std::env::var_os("CRUISE_CI_VERIFY_SCRIPT").filter(|value| !value.is_empty())
    {
        let worktree = std::env::current_dir()?.canonicalize()?;
        return backend::jcode::verify_ci_snapshot(candidate, &worktree, script.as_ref())
            .map_err(error::CruiseError::Other);
    }
    let _ = candidate;
    eprintln!(
        "Warning: skipping host verification of the CI repair (requires Bubblewrap and CRUISE_CI_VERIFY_SCRIPT); CI verifies the pushed head"
    );
    Ok(())
}
