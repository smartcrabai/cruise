use clap::{Parser, Subcommand};

pub(crate) const DEFAULT_RATE_LIMIT_RETRIES: usize = 5;
pub(crate) const PLAN_STDIN_SENTINEL: &str = "stdin";

#[derive(Parser, Debug)]
#[command(
    name = "cruise",
    version,
    about = "YAML-driven coding agent workflow orchestrator",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Create a plan in the background and return immediately.
    ///
    /// Pass `stdin` to read the task description from piped stdin explicitly.
    #[arg(long, value_name = "INPUT", conflicts_with = "input")]
    pub plan: Option<String>,

    /// Use the input directly as the plan, skipping LLM generation.
    ///
    /// Works with either `--plan <INPUT>` (background) or the positional
    /// `[INPUT]` (foreground); the given text becomes the plan verbatim.
    #[arg(long)]
    pub skip_planning: bool,

    /// GitHub repository (owner/repository) to clone into a temporary
    /// directory for planning and execution. The clone is removed after the
    /// plan is approved and again after the PR has been created.
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,

    /// Attach an image file (png/jpg/jpeg/webp/gif) to the planning input.
    /// Forwarded to the `plan` subcommand. Can be repeated.
    #[arg(long = "image", value_name = "PATH")]
    pub images: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Initial input (legacy: no subcommand is treated as `plan`).
    #[arg(conflicts_with = "plan")]
    pub input: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Create an implementation plan for a task.
    Plan(PlanArgs),
    #[command(hide = true)]
    PlanWorker(PlanWorkerArgs),
    /// Save a task description as a draft, without generating a plan.
    Draft(DraftArgs),
    /// Execute a planned session.
    Run(RunArgs),
    /// List and manage sessions interactively.
    List(ListArgs),
    /// Remove sessions with closed/merged PRs.
    Clean(CleanArgs),
    /// Show or update application-level configuration (`~/.config/cruise/config.json`).
    Config(ConfigArgs),
    /// Execute the workflow config directly in the current directory (no plan, no worktree, no PR).
    Exec(ExecArgs),
}

#[derive(Parser, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are naturally boolean"
)]
pub struct PlanArgs {
    /// Task description.
    pub input: Option<String>,

    /// Path to the workflow config file.
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Print the plan step without executing it.
    #[arg(long)]
    pub dry_run: bool,

    /// Use the input directly as the plan; skip LLM-based planning.
    #[arg(long)]
    pub skip_planning: bool,

    /// "Grill me" planning: interview you one question at a time until the design
    /// is fully pinned down, then write the plan. Requires the SDK backend and an
    /// interactive terminal (errors otherwise). Conflicts with `--skip-planning`.
    #[arg(long, conflicts_with = "skip_planning")]
    pub grill: bool,

    /// Disable interactive planning tools (`submit_plan`/`update_plan`/`ask_user`)
    /// for this session, even if the workflow config has `interactive_planning: true`.
    /// The agent writes `plan.md` directly instead. Useful when using
    /// tool-incapable providers. Conflicts with `--grill`.
    #[arg(long, conflicts_with = "grill")]
    pub no_interactive_planning: bool,

    /// GitHub repository (owner/repository) to clone into a temporary
    /// directory for planning and execution. The clone is removed after the
    /// plan is approved and again after the PR has been created.
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,

    /// Maximum number of rate-limit retries per LLM call.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT_RETRIES)]
    pub rate_limit_retries: usize,

    /// Attach an image file (png/jpg/jpeg/webp/gif) to the planning input.
    /// Can be repeated. Images are also auto-detected when their paths are
    /// dragged onto / pasted into the interactive prompt.
    #[arg(long = "image", value_name = "PATH")]
    pub images: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct DraftArgs {
    /// Task description.
    pub input: Option<String>,

    /// Path to the workflow config file.
    #[arg(short = 'c', long)]
    pub config: Option<String>,
}

#[derive(Parser, Debug)]
pub struct PlanWorkerArgs {
    /// Session ID whose plan should be generated.
    #[arg(long)]
    pub session: String,

    /// Maximum number of rate-limit retries per LLM call.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT_RETRIES)]
    pub rate_limit_retries: usize,
}

#[derive(Parser, Debug)]
#[expect(clippy::struct_excessive_bools)]
pub struct RunArgs {
    /// Session ID to execute (if omitted, picks from pending sessions).
    #[arg(conflicts_with = "all")]
    pub session: Option<String>,

    /// Run all planned sessions sequentially.
    #[arg(long)]
    pub all: bool,

    /// Maximum number of times a single loop edge may be traversed.
    ///
    /// When omitted, falls back to the workflow config's top-level `max_retries`
    /// if set, otherwise defaults to 3. An explicitly-passed flag always wins
    /// over the config value.
    #[arg(long)]
    pub max_retries: Option<usize>,

    /// Maximum number of rate-limit retries per step.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT_RETRIES)]
    pub rate_limit_retries: usize,

    /// Print the workflow flow without executing it.
    #[arg(long)]
    pub dry_run: bool,
    /// Force-enable post-PR worktree+branch cleanup for this run.
    #[arg(long, conflicts_with = "no_cleanup_after_pr")]
    pub cleanup_after_pr: bool,

    /// Force-disable post-PR worktree+branch cleanup for this run.
    #[arg(long, conflicts_with = "cleanup_after_pr")]
    pub no_cleanup_after_pr: bool,
}
impl RunArgs {
    /// Convert the CLI cleanup flags into an `Option<bool>` override.
    /// Both flags default to `false`; when neither is set, returns `None`.
    #[must_use]
    pub fn cleanup_after_pr_override(&self) -> Option<bool> {
        if self.cleanup_after_pr {
            Some(true)
        } else if self.no_cleanup_after_pr {
            Some(false)
        } else {
            None
        }
    }
}

#[derive(Parser, Debug)]
pub struct CleanArgs {}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Output all sessions as a JSON array to stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct ConfigArgs {
    /// Set the maximum number of sessions to run concurrently in `run --all` mode.
    ///
    /// Must be >= 1. Omit to show the current configuration.
    #[arg(long, value_name = "N")]
    pub set_parallelism: Option<usize>,
}

#[derive(Parser, Debug)]
pub struct ExecArgs {
    /// Task description bound to {input}. Optional if your config doesn't reference {input}.
    pub input: Option<String>,

    /// Path to the workflow config file.
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Maximum number of times a single loop edge may be traversed.
    ///
    /// When omitted, falls back to the workflow config's top-level `max_retries`
    /// if set, otherwise defaults to 3. An explicitly-passed flag always wins
    /// over the config value.
    #[arg(long)]
    pub max_retries: Option<usize>,

    /// Maximum number of rate-limit retries per step.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT_RETRIES)]
    pub rate_limit_retries: usize,

    /// Print the workflow flow without executing it.
    #[arg(long)]
    pub dry_run: bool,
}

pub fn parse_cli() -> Cli {
    let mut cli = Cli::parse();

    // Backward compat: no subcommand + stdin pipe -> read input from stdin.
    if cli.command.is_none()
        && cli.plan.is_none()
        && cli.input.is_none()
        && !std::io::IsTerminal::is_terminal(&std::io::stdin())
    {
        use std::io::Read;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok();
        let trimmed = input.trim().to_string();
        if !trimmed.is_empty() {
            cli.input = Some(trimmed);
        }
    }

    cli
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_verify() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_plan_subcommand_with_input() {
        let cli = Cli::parse_from(["cruise", "plan", "add feature X"]);
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert_eq!(args.input, Some("add feature X".to_string()));
                assert!(!args.dry_run);
                assert_eq!(args.rate_limit_retries, DEFAULT_RATE_LIMIT_RETRIES);
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_subcommand_with_config() {
        let cli = Cli::parse_from(["cruise", "plan", "-c", "my.yaml", "task"]);
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert_eq!(args.config, Some("my.yaml".to_string()));
                assert_eq!(args.input, Some("task".to_string()));
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_subcommand_dry_run() {
        let cli = Cli::parse_from(["cruise", "plan", "--dry-run", "task"]);
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert!(args.dry_run);
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_grill_defaults_to_false() {
        let cli = Cli::parse_from(["cruise", "plan", "task"]);
        match cli.command {
            Some(Commands::Plan(args)) => assert!(!args.grill),
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_grill_flag_sets_true() {
        let cli = Cli::parse_from(["cruise", "plan", "--grill", "task"]);
        match cli.command {
            Some(Commands::Plan(args)) => assert!(args.grill),
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_no_interactive_planning_defaults_to_false() {
        let cli = Cli::parse_from(["cruise", "plan", "task"]);
        match cli.command {
            Some(Commands::Plan(args)) => assert!(!args.no_interactive_planning),
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_no_interactive_planning_flag_sets_true() {
        let cli = Cli::parse_from(["cruise", "plan", "--no-interactive-planning", "task"]);
        match cli.command {
            Some(Commands::Plan(args)) => assert!(args.no_interactive_planning),
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_no_interactive_planning_conflicts_with_grill() {
        let result = Cli::try_parse_from([
            "cruise",
            "plan",
            "--no-interactive-planning",
            "--grill",
            "task",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_plan_grill_conflicts_with_skip_planning() {
        // `--grill` (interview to build the plan) is incompatible with
        // `--skip-planning` (use input verbatim as the plan).
        let result = Cli::try_parse_from(["cruise", "plan", "--grill", "--skip-planning", "task"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_subcommand_defaults() {
        let cli = Cli::parse_from(["cruise", "run"]);
        match cli.command {
            Some(Commands::Run(args)) => {
                assert_eq!(args.session, None);
                assert_eq!(
                    args.max_retries, None,
                    "--max-retries omitted should parse to None (config/default resolved later)"
                );
                assert_eq!(args.rate_limit_retries, DEFAULT_RATE_LIMIT_RETRIES);
                assert!(!args.dry_run);
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_with_session() {
        let cli = Cli::parse_from(["cruise", "run", "20260306143000"]);
        match cli.command {
            Some(Commands::Run(args)) => {
                assert_eq!(args.session, Some("20260306143000".to_string()));
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_flags() {
        let cli = Cli::parse_from([
            "cruise",
            "run",
            "--max-retries",
            "20",
            "--rate-limit-retries",
            "3",
        ]);
        match cli.command {
            Some(Commands::Run(args)) => {
                assert_eq!(args.max_retries, Some(20));
                assert_eq!(args.rate_limit_retries, 3);
            }
            _ => panic!("expected Run subcommand"),
        }
    }
    #[test]
    fn test_run_subcommand_cleanup_after_pr_flag_defaults_to_false() {
        // Given: run subcommand with no cleanup flags
        let cli = Cli::parse_from(["cruise", "run"]);
        // When/Then: both flags default to false and override is None
        match cli.command {
            Some(Commands::Run(args)) => {
                assert!(!args.cleanup_after_pr);
                assert!(!args.no_cleanup_after_pr);
                assert_eq!(args.cleanup_after_pr_override(), None);
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_cleanup_after_pr_flag_sets_override_true() {
        // Given: --cleanup-after-pr is present
        let cli = Cli::parse_from(["cruise", "run", "--cleanup-after-pr"]);
        // When/Then: override is Some(true)
        match cli.command {
            Some(Commands::Run(args)) => {
                assert!(args.cleanup_after_pr);
                assert!(!args.no_cleanup_after_pr);
                assert_eq!(args.cleanup_after_pr_override(), Some(true));
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_no_cleanup_after_pr_flag_sets_override_false() {
        // Given: --no-cleanup-after-pr is present
        let cli = Cli::parse_from(["cruise", "run", "--no-cleanup-after-pr"]);
        // When/Then: override is Some(false)
        match cli.command {
            Some(Commands::Run(args)) => {
                assert!(!args.cleanup_after_pr);
                assert!(args.no_cleanup_after_pr);
                assert_eq!(args.cleanup_after_pr_override(), Some(false));
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_cleanup_flags_conflict() {
        // Given: both cleanup flags are present
        let result = Cli::try_parse_from([
            "cruise",
            "run",
            "--cleanup-after-pr",
            "--no-cleanup-after-pr",
        ]);
        // Then: parsing fails because the flags are mutually exclusive
        assert!(result.is_err());
    }

    #[test]
    fn test_root_plan_flag_with_inline_input_parses() {
        // Given / When: the new root-level --plan flag is used with inline text
        let cli = Cli::try_parse_from(["cruise", "--plan", "add feature X"])
            .unwrap_or_else(|e| panic!("expected --plan to parse successfully: {e}"));

        // Then: it stays on the root command path instead of falling back to legacy positional input
        assert!(cli.command.is_none(), "expected no subcommand: {cli:?}");
        assert_eq!(cli.plan, Some("add feature X".to_string()));
        assert_eq!(cli.input, None, "legacy positional input should stay empty");
    }

    #[test]
    fn test_root_plan_flag_with_stdin_literal_parses() {
        // Given / When: the new root-level --plan flag is used with the explicit stdin sentinel
        let cli = Cli::try_parse_from(["cruise", "--plan", "stdin"])
            .unwrap_or_else(|e| panic!("expected --plan stdin to parse successfully: {e}"));

        // Then: it is accepted as a root invocation
        assert!(cli.command.is_none(), "expected no subcommand: {cli:?}");
        assert_eq!(cli.plan, Some(PLAN_STDIN_SENTINEL.to_string()));
        assert_eq!(cli.input, None, "legacy positional input should stay empty");
    }

    #[test]
    fn test_list_subcommand() {
        let cli = Cli::parse_from(["cruise", "list"]);
        assert!(matches!(cli.command, Some(Commands::List(_))));
    }

    #[test]
    fn test_list_subcommand_json_flag_defaults_to_false() {
        let cli = Cli::parse_from(["cruise", "list"]);
        match cli.command {
            Some(Commands::List(args)) => {
                assert!(!args.json, "--json should default to false");
            }
            _ => panic!("expected List subcommand"),
        }
    }

    #[test]
    fn test_list_subcommand_json_flag_is_true_with_flag() {
        let cli = Cli::parse_from(["cruise", "list", "--json"]);
        match cli.command {
            Some(Commands::List(args)) => {
                assert!(args.json, "--json should be true");
            }
            _ => panic!("expected List subcommand"),
        }
    }

    #[test]
    fn test_clean_subcommand_default() {
        let cli = Cli::parse_from(["cruise", "clean"]);
        assert!(matches!(cli.command, Some(Commands::Clean(_))));
    }

    #[test]
    fn test_backward_compat_no_subcommand() {
        let cli = Cli::parse_from(["cruise", "add hello world"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.input, Some("add hello world".to_string()));
    }

    #[test]
    fn test_no_args() {
        let cli = Cli::parse_from(["cruise"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.input, None);
    }

    #[test]
    fn test_run_subcommand_all_flag() {
        // Given: only the --all flag is specified
        let cli = Cli::parse_from(["cruise", "run", "--all"]);
        // When/Then: all=true, session=None
        match cli.command {
            Some(Commands::Run(args)) => {
                assert!(args.all, "--all should be true");
                assert_eq!(args.session, None);
                assert!(!args.dry_run);
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_all_flag_default_is_false() {
        // Given: run subcommand with no flags
        let cli = Cli::parse_from(["cruise", "run"]);
        // When/Then: all defaults to false
        match cli.command {
            Some(Commands::Run(args)) => {
                assert!(!args.all, "--all should default to false");
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    #[test]
    fn test_run_subcommand_all_with_dry_run() {
        // Given: combination of --all and --dry-run
        let cli = Cli::parse_from(["cruise", "run", "--all", "--dry-run"]);
        // When/Then: both flags are active
        match cli.command {
            Some(Commands::Run(args)) => {
                assert!(args.all);
                assert!(args.dry_run);
                assert_eq!(args.session, None);
            }
            _ => panic!("expected Run subcommand"),
        }
    }

    // -- Config subcommand ----------------------------------------------------

    #[test]
    fn test_config_subcommand_no_flags_shows_current_config() {
        // Given: `cruise config` with no arguments
        let cli = Cli::parse_from(["cruise", "config"]);
        // When/Then: Config subcommand with no set_parallelism (show mode)
        match cli.command {
            Some(Commands::Config(args)) => {
                assert_eq!(
                    args.set_parallelism, None,
                    "no flags means show-only mode (set_parallelism is None)"
                );
            }
            _ => panic!("expected Config subcommand"),
        }
    }

    #[test]
    fn test_config_subcommand_set_parallelism_parses_value() {
        // Given: `cruise config --set-parallelism 4`
        let cli = Cli::parse_from(["cruise", "config", "--set-parallelism", "4"]);
        // When/Then: set_parallelism is Some(4)
        match cli.command {
            Some(Commands::Config(args)) => {
                assert_eq!(
                    args.set_parallelism,
                    Some(4),
                    "expected set_parallelism = Some(4)"
                );
            }
            _ => panic!("expected Config subcommand"),
        }
    }

    #[test]
    fn test_config_subcommand_set_parallelism_one() {
        // Given: `cruise config --set-parallelism 1` -- minimum valid value
        let cli = Cli::parse_from(["cruise", "config", "--set-parallelism", "1"]);
        match cli.command {
            Some(Commands::Config(args)) => {
                assert_eq!(args.set_parallelism, Some(1));
            }
            _ => panic!("expected Config subcommand"),
        }
    }

    #[test]
    fn test_config_subcommand_is_registered_in_cli_verify() {
        // Given/When/Then: clap validates the full command definition including Config
        Cli::command().debug_assert();
    }

    // -- Exec subcommand -------------------------------------------------------

    #[test]
    fn test_exec_subcommand_with_input_and_config() {
        // Given: exec subcommand with explicit config and positional input
        let cli = Cli::parse_from(["cruise", "exec", "-c", "my.yaml", "task"]);
        // When/Then: both args are captured
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert_eq!(args.input, Some("task".to_string()));
                assert_eq!(args.config, Some("my.yaml".to_string()));
            }
            _ => panic!("expected Exec subcommand"),
        }
    }

    #[test]
    fn test_exec_subcommand_defaults() {
        // Given: exec subcommand with no optional flags
        let cli = Cli::parse_from(["cruise", "exec"]);
        // When/Then: all fields take their defaults
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert_eq!(args.input, None);
                assert_eq!(args.config, None);
                assert_eq!(
                    args.max_retries, None,
                    "--max-retries omitted should parse to None (config/default resolved later)"
                );
                assert_eq!(args.rate_limit_retries, DEFAULT_RATE_LIMIT_RETRIES);
                assert!(!args.dry_run);
            }
            _ => panic!("expected Exec subcommand"),
        }
    }

    #[test]
    fn test_exec_subcommand_dry_run_flag() {
        // Given: exec subcommand with --dry-run
        let cli = Cli::parse_from(["cruise", "exec", "--dry-run"]);
        // When/Then: dry_run is true
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert!(args.dry_run);
                assert_eq!(args.input, None);
            }
            _ => panic!("expected Exec subcommand"),
        }
    }

    #[test]
    fn test_exec_subcommand_custom_retries() {
        // Given: exec with explicit retry counts
        let cli = Cli::parse_from([
            "cruise",
            "exec",
            "--max-retries",
            "5",
            "--rate-limit-retries",
            "2",
        ]);
        // When/Then: custom values are parsed
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert_eq!(args.max_retries, Some(5));
                assert_eq!(args.rate_limit_retries, 2);
            }
            _ => panic!("expected Exec subcommand"),
        }
    }

    // -- skip-planning flag on plan subcommand ---------------------------------

    #[test]
    fn test_plan_skip_planning_defaults_to_false() {
        // Given: plan subcommand with no --skip-planning flag
        let cli = Cli::parse_from(["cruise", "plan", "my task"]);
        // When/Then: skip_planning is false by default
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert!(
                    !args.skip_planning,
                    "--skip-planning should default to false"
                );
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_skip_planning_flag_sets_true() {
        // Given: --skip-planning flag is present on plan subcommand
        let cli = Cli::parse_from(["cruise", "plan", "--skip-planning", "my task"]);
        // When/Then: skip_planning is true
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert!(
                    args.skip_planning,
                    "--skip-planning should be true when flag is present"
                );
                assert_eq!(args.input, Some("my task".to_string()));
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_skip_planning_combined_with_config() {
        // Given: --skip-planning combined with -c config and positional input
        let cli = Cli::parse_from([
            "cruise",
            "plan",
            "--skip-planning",
            "-c",
            "my.yaml",
            "do task",
        ]);
        // When/Then: all fields captured
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert!(args.skip_planning);
                assert_eq!(args.config, Some("my.yaml".to_string()));
                assert_eq!(args.input, Some("do task".to_string()));
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    // -- skip-planning flag on root --plan form --------------------------------

    #[test]
    fn test_root_skip_planning_defaults_to_false() {
        // Given: --plan flag with no --skip-planning
        let cli = Cli::try_parse_from(["cruise", "--plan", "task"])
            .unwrap_or_else(|e| panic!("parse error: {e}"));
        // When/Then: root-level skip_planning defaults to false
        assert!(
            !cli.skip_planning,
            "root-level --skip-planning should default to false"
        );
    }

    #[test]
    fn test_root_skip_planning_with_plan_flag() {
        // Given: --plan and --skip-planning at root level
        let cli = Cli::try_parse_from(["cruise", "--plan", "task text", "--skip-planning"])
            .unwrap_or_else(|e| panic!("expected --plan --skip-planning to parse: {e}"));
        // When/Then: both flags are captured on the root struct
        assert_eq!(cli.plan, Some("task text".to_string()));
        assert!(
            cli.skip_planning,
            "root-level --skip-planning should be true"
        );
    }

    #[test]
    fn test_root_skip_planning_with_positional_input_parses() {
        // Given: --skip-planning combined with positional [INPUT] (no --plan)
        let cli = Cli::try_parse_from(["cruise", "--skip-planning", "task text"])
            .unwrap_or_else(|e| panic!("expected --skip-planning [INPUT] to parse: {e}"));
        // When/Then: skip_planning is true and the text lands on the positional input
        assert!(cli.command.is_none(), "expected no subcommand: {cli:?}");
        assert!(
            cli.skip_planning,
            "root-level --skip-planning should be true"
        );
        assert_eq!(cli.input, Some("task text".to_string()));
        assert_eq!(cli.plan, None, "--plan should stay empty");
    }

    // -- repo flag --------------------------------------------------------------

    #[test]
    fn test_plan_image_flag_collects_paths() {
        let cli = Cli::parse_from([
            "cruise",
            "plan",
            "--image",
            "/tmp/a.png",
            "--image",
            "/tmp/b.jpg",
            "task",
        ]);
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert_eq!(args.images, vec!["/tmp/a.png", "/tmp/b.jpg"]);
                assert_eq!(args.input, Some("task".to_string()));
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_root_plan_image_flag_collects_paths() {
        let cli = Cli::try_parse_from([
            "cruise",
            "--plan",
            "task",
            "--image",
            "/tmp/a.png",
            "--image",
            "/tmp/b.jpg",
        ])
        .unwrap_or_else(|e| panic!("expected --plan --image to parse: {e}"));
        assert_eq!(cli.images, vec!["/tmp/a.png", "/tmp/b.jpg"]);
    }

    #[test]
    fn test_plan_repo_flag_parses() {
        // Given: plan subcommand with --repo owner/repo
        let cli = Cli::parse_from(["cruise", "plan", "--repo", "owner/repo", "my task"]);
        // When/Then: the repo spec and input are captured
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert_eq!(args.repo, Some("owner/repo".to_string()));
                assert_eq!(args.input, Some("my task".to_string()));
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_plan_repo_defaults_to_none() {
        let cli = Cli::parse_from(["cruise", "plan", "my task"]);
        match cli.command {
            Some(Commands::Plan(args)) => {
                assert_eq!(args.repo, None, "--repo should default to None");
            }
            _ => panic!("expected Plan subcommand"),
        }
    }

    #[test]
    fn test_root_plan_flag_with_repo_parses() {
        // Given: background --plan combined with --repo at root level
        let cli = Cli::try_parse_from(["cruise", "--plan", "task", "--repo", "owner/repo"])
            .unwrap_or_else(|e| panic!("expected --plan --repo to parse: {e}"));
        // When/Then: both are captured on the root struct
        assert_eq!(cli.plan, Some("task".to_string()));
        assert_eq!(cli.repo, Some("owner/repo".to_string()));
    }

    #[test]
    fn test_root_skip_planning_alone_parses() {
        // Given: --skip-planning with neither --plan nor positional input
        let cli = Cli::try_parse_from(["cruise", "--skip-planning"])
            .unwrap_or_else(|e| panic!("expected bare --skip-planning to parse: {e}"));
        // When/Then: it is accepted (input is resolved later via stdin/prompt)
        assert!(cli.skip_planning);
        assert_eq!(cli.input, None);
        assert_eq!(cli.plan, None);
    }
}
