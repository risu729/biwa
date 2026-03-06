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
#![cfg_attr(
	test,
	allow(clippy::panic_in_result_fn, reason = "color_eyre handles panics")
)]

#[expect(clippy::disallowed_types, reason = "This is the crate's central Result type definition.")]
pub type Result<T> = color_eyre::Result<T>;

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
async fn main() -> Result<()> {
	color_eyre::install()?;
	cli::run().await?;
	Ok(())
}

#[cfg(test)]
#[ctor::ctor]
fn init_test_env() {
	#[expect(
		clippy::unused_result_ok,
		reason = "Multiple tests may attempt to initialize the global error handler."
	)]
	color_eyre::install().ok();
}
