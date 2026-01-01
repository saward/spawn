use anyhow::Result;
use clap::Parser;
use opendal::services::Fs;
use opendal::Operator;
use spawn::cli::{run_cli, Cli};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let service = Fs::default().root(".");
    let config_fs = Operator::new(service)?.finish();

    let _outcome = run_cli(cli, &config_fs, None::<fn(&str) -> Result<Operator>>).await?;

    Ok(())
}
