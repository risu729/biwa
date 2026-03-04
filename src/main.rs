pub use eyre::Result;

mod cli;
mod config;
mod ssh;

#[tokio::main]
async fn main() -> Result<()> {
	cli::run().await?;
	Ok(())
}
