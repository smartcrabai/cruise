use clap::{Parser, Subcommand};

pub(crate) const DEFAULT_MAX_RETRIES: usize = 10;
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
    /// Execute a planned session.
    Run(RunArgs),
    /// List and manage sessions interactively.
    List(ListArgs),
    /// Remove sessions with closed/merged PRs.
    Clean(CleanArgs),
    /// Show or update application-level configuration (`~/.config/cruise/config.json`).
    Config(ConfigArgs),
}

#[derive(Parser, Debug)]
pub struct PlanArgs {
    /// Task description.
    pub input: Option<String>,

    /// Path to the workflow config file.
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Print the plan step without executing it.
    #[arg(long)]
    pub dry_run: bool,

    /// Maximum number of rate-limit retries per LLM call.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT_RETRIES)]
    pub rate_limit_retries: usize,
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
pub struct RunArgs {
    /// Session ID to execute (if omitted, picks from pending sessions).
    #[arg(conflicts_with = "all")]
    pub session: Option<String>,

    /// Run all planned sessions sequentially.
    #[arg(long)]
    pub all: bool,

    /// Maximum number of times a single loop edge may be traversed.
    #[arg(long, default_value_t = DEFAULT_MAX_RETRIES)]
    pub max_retries: usize,

    /// Maximum number of rate-limit retries per step.
    #[arg(long, default_value_t = DEFAULT_RATE_LIMIT_RETRIES)]
    pub rate_limit_retries: usize,

    /// Print the workflow flow without executing it.
    #[arg(long)]
    pub dry_run: bool,
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
    fn test_run_subcommand_defaults() {
        let cli = Cli::parse_from(["cruise", "run"]);
        match cli.command {
            Some(Commands::Run(args)) => {
                assert_eq!(args.session, None);
                assert_eq!(args.max_retries, DEFAULT_MAX_RETRIES);
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
                assert_eq!(args.max_retries, 20);
                assert_eq!(args.rate_limit_retries, 3);
            }
            _ => panic!("expected Run subcommand"),
        }
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
}
