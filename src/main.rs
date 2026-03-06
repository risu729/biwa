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
#[cfg(test)]
mod testing;
/// UI components.
mod ui;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
	color_eyre::install()?;
	cli::run().await?;
	Ok(())
}

#[cfg(test)]
#[ctor::ctor]
fn init_test_env() {
	color_eyre::install().ok();
}
