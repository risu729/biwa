#![warn(clippy::all, clippy::pedantic, clippy::cargo)]

pub use eyre::Result;

mod cli;
mod config;
mod ssh;
mod ui;

#[tokio::main]
async fn main() -> Result<()> {
	cli::run().await?;
	Ok(())
}
