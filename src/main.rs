use anyhow::Result;
use clap::Parser;
use spawn::cli::{run_cli, Cli};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _outcome = run_cli(cli).await?;

    Ok(())
}
