//! DSvn Administration CLI

mod dump;
mod dump_format;
mod load;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::BufReader;

#[derive(Parser, Debug)]
#[command(name = "dsvn-admin")]
#[command(author = "DSvn Contributors")]
#[command(version = "0.1.0")]
#[command(about = "DSvn repository administration and dump file tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a new repository
    Init { path: String },

    /// Load SVN dump file into repository
    Load {
        #[arg(short, long)]
        file: String,
        #[arg(short, long)]
        repo: Option<String>,
    },

    /// Dump repository to SVN dump format
    Dump {
        #[arg(short, long)]
        repo: String,
        #[arg(short, long)]
        output: String,
        #[arg(short, long)]
        start: Option<u64>,
        #[arg(short, long)]
        end: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => {
            println!("Initializing repository at {}", path);
            std::fs::create_dir_all(&path)?;
            std::fs::create_dir_all(format!("{}/hot", path))?;
            std::fs::create_dir_all(format!("{}/warm", path))?;
            std::fs::create_dir_all(format!("{}/conf", path))?;
            println!("Repository initialized successfully");
        }

        Commands::Load { file, repo: _ } => {
            println!("Loading SVN dump file: {}", file);
            let repository = dsvn_core::Repository::new();
            repository.initialize().await?;

            if file == "-" {
                let reader = BufReader::new(std::io::stdin());
                load::load_dump_file(&repository, reader).await?;
            } else {
                let file_obj = File::open(&file)?;
                let reader = BufReader::new(file_obj);
                load::load_dump_file(&repository, reader).await?;
            }
            println!("Load complete!");
        }

        Commands::Dump { repo: _repo, output: _output, start: _start, end: _end } => {
            println!("Dump functionality coming soon");
        }
    }

    Ok(())
}
