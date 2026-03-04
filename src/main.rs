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

mod cli;
mod config;
mod ssh;

#[tokio::main]
async fn main() -> eyre::Result<()> {
	cli::run().await?;
	Ok(())
}
