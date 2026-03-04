#![cfg_attr(
	test,
	allow(clippy::unwrap_used, reason = "too verbose to use expect in tests")
)]
#![cfg_attr(
	test,
	allow(
		clippy::shadow_unrelated,
		reason = "some tests have repeated variable names"
	)
)]

/// CLI commands and parsing.
mod cli;
/// Configuration loading and definitions.
mod config;
/// SSH execution logic.
mod ssh;
mod ui;

#[tokio::main]
async fn main() -> eyre::Result<()> {
	cli::run().await?;
	Ok(())
}
