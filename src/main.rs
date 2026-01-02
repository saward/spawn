use anyhow::Result;
use clap::Parser;
use opendal::services::Fs;
use opendal::Operator;
use spawn::cli::{run_cli, Cli, Outcome};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let service = Fs::default().root(".");
    let config_fs = Operator::new(service)?.finish();

    let outcome = run_cli(cli, &config_fs).await?;

    match outcome {
        Outcome::BuiltMigration { content } => {
            println!("{}", content);
        }
        Outcome::AppliedMigrations => {
            println!("All migrations applied successfully.");
        }
        Outcome::NewMigration(name) => {
            println!("New migration created: {}", name);
        }
        Outcome::Unimplemented => {
            println!("Unimplemented command.");
        }
    }

    Ok(())
}
