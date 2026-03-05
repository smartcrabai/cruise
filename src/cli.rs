use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "cruise",
    version,
    about = "YAML-driven coding agent workflow orchestrator"
)]
pub struct Args {
    /// Initial input passed to the workflow.
    pub input: Option<String>,

    /// Path to the workflow config file.
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Step name to start from (for resuming mid-workflow).
    #[arg(long)]
    pub from: Option<String>,

    /// Maximum number of times a single loop edge may be traversed.
    #[arg(long, default_value = "10")]
    pub max_retries: usize,

    /// Maximum number of rate-limit retries per step.
    #[arg(long, default_value = "5")]
    pub rate_limit_retries: usize,

    /// Print the workflow flow without executing it.
    #[arg(long)]
    pub dry_run: bool,

    /// Run workflow in an isolated git worktree.
    #[arg(long)]
    pub worktree: bool,

    /// Keep the worktree after workflow completes (default: auto-delete).
    #[arg(long)]
    pub keep_worktree: bool,

    /// Skip worktree resume detection; always create a new worktree.
    #[arg(long)]
    pub new_worktree: bool,
}

pub fn parse_args() -> Args {
    let mut args = Args::parse();

    // Read from stdin when no INPUT argument is given and stdin is a pipe.
    if args.input.is_none() && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Read;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok();
        let trimmed = input.trim().to_string();
        if !trimmed.is_empty() {
            args.input = Some(trimmed);
        }
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_args_parse_with_input() {
        let args = Args::parse_from(["cruise", "hello world"]);
        assert_eq!(args.input, Some("hello world".to_string()));
        assert_eq!(args.config, None);
        assert_eq!(args.max_retries, 10);
        assert_eq!(args.rate_limit_retries, 5);
        assert!(!args.dry_run);
    }

    #[test]
    fn test_args_parse_with_config() {
        let args = Args::parse_from(["cruise", "-c", "my.yaml", "task"]);
        assert_eq!(args.config, Some("my.yaml".to_string()));
        assert_eq!(args.input, Some("task".to_string()));
    }

    #[test]
    fn test_args_parse_with_from() {
        let args = Args::parse_from(["cruise", "--from", "implement", "task"]);
        assert_eq!(args.from, Some("implement".to_string()));
    }

    #[test]
    fn test_args_parse_max_retries() {
        let args = Args::parse_from(["cruise", "--max-retries", "20"]);
        assert_eq!(args.max_retries, 20);
    }

    #[test]
    fn test_args_parse_rate_limit_retries() {
        let args = Args::parse_from(["cruise", "--rate-limit-retries", "3"]);
        assert_eq!(args.rate_limit_retries, 3);
    }

    #[test]
    fn test_args_parse_dry_run() {
        let args = Args::parse_from(["cruise", "--dry-run", "task"]);
        assert!(args.dry_run);
    }

    #[test]
    fn test_args_parse_worktree() {
        let args = Args::parse_from(["cruise", "--worktree", "task"]);
        assert!(args.worktree);
        assert!(!args.keep_worktree);
    }

    #[test]
    fn test_args_parse_keep_worktree() {
        let args = Args::parse_from(["cruise", "--worktree", "--keep-worktree", "task"]);
        assert!(args.worktree);
        assert!(args.keep_worktree);
    }

    #[test]
    fn test_args_parse_no_input() {
        let args = Args::parse_from(["cruise"]);
        assert_eq!(args.input, None);
    }

    #[test]
    fn test_cli_verify() {
        Args::command().debug_assert();
    }
}
