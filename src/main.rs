#![warn(clippy::all, clippy::pedantic, clippy::cargo)]

pub use eyre::Result;

mod cli;
pub mod config;
mod ssh;

#[tokio::main]
async fn main() -> Result<()> {
	cli::run().await?;
	Ok(())
}
