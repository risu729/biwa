use crate::config::types::Config;
use crate::ssh::sync::sync_project;
use clap::Args;
use std::env;

/// Synchronize local project files to the remote server.
#[derive(Args, Debug)]
#[clap(visible_alias = "s")]
pub(super) struct Sync;

impl Sync {
	/// Run the sync logic.
	pub async fn run(self, config: &Config, quiet: bool, _silent: bool) -> eyre::Result<()> {
		let current_dir = env::current_dir()?;
		// Assuming current directory is project root or we can just use current directory.
		sync_project(config, &current_dir, quiet).await?;
		Ok(())
	}
}
