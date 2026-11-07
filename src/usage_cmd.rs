use crate::usage::{
    self,
    summary::{self, GroupDim},
};
use clap::{Args, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UsageFormat {
    Text,
    Json,
    Csv,
}
#[derive(Args, Debug)]
pub struct UsageArgs {
    /// Comma-separated grouping dimensions.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "step,backend,model"
    )]
    pub by: Vec<GroupDim>,
    /// Filter by exact session ID.
    #[arg(long)]
    pub session: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value = "text")]
    pub format: UsageFormat,
}
/// # Errors
/// Returns an error when the usage file cannot be read or rendered.
pub fn run(args: &UsageArgs) -> crate::error::Result<()> {
    let summary = summary::summarize(&usage::usage_path()?, &args.by, args.session.as_deref())?;
    println!(
        "{}",
        match args.format {
            UsageFormat::Text => summary.text(),
            UsageFormat::Json => summary.json()?,
            UsageFormat::Csv => summary.csv(),
        }
    );
    Ok(())
}
