//! DSvn Administration CLI

mod dump;
mod dump_format;
mod load;

use anyhow::Result;
use clap::{Parser, Subcommand};
use dsvn_core::DiskRepository;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

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
        repo: String,
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
            let repo = DiskRepository::open(Path::new(&path))?;
            repo.initialize().await?;
            println!("Repository initialized successfully (UUID: {})", repo.uuid());
        }

        Commands::Load { file, repo } => {
            println!("Loading SVN dump file: {}", file);
            let repository = DiskRepository::open(Path::new(&repo))?;
            repository.initialize().await?;
            let repository = Arc::new(repository);

            if file == "-" {
                let reader = BufReader::new(std::io::stdin());
                load::load_dump_file(repository, reader).await?;
            } else {
                let file_obj = File::open(&file)?;
                let reader = BufReader::new(file_obj);
                load::load_dump_file(repository, reader).await?;
            }
        }

        Commands::Dump { repo, output: _output, start: _start, end: _end } => {
            let _repository = DiskRepository::open(Path::new(&repo))?;
            println!("Dump functionality coming soon");
        }
    }

    Ok(())
}
