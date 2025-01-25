use std::path::PathBuf;

use anyhow::{Context, Result};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Turn debugging information on
    #[arg(short, long)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Generate {
        #[arg(short, long)]
        script: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.debug {
        false => println!("Debug mode is off"),
        true => println!("Debug mode is on"),
    }

    match &cli.command {
        Some(Commands::Generate { script }) => {
            if let Some(script) = script {
                println!("Generating migration script for file '{}'", script);
                generate(script)?;
            } else {
                println!("No script specified, generating all")
            }
        }
        None => {}
    }

    Ok(())
}

// Opens the specified script file and generates a migration script, compiled
// using minijinja.
fn generate(script: &String) -> Result<()> {
    let path = PathBuf::from(format!("./static/example/templates/{}", script));
    let contents = std::fs::read_to_string(path).context("could not open script")?;

    println!("{}", contents);
    Ok(())
}
