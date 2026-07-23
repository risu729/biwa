use crate::Result;
use crate::cli::clean::spawn_background_cleanup;
use crate::cli::transfer::{TransferArgs, record_connection_use};
use crate::config::types::Config;
use crate::ssh::exec::connect;
use crate::ssh::sync::pull_project;
use clap::Args;
use tracing::warn;

/// Mirror remote project files into the local root.
#[derive(Args, Debug)]
pub(super) struct Pull {
	/// Project transfer options.
	#[clap(flatten)]
	args: TransferArgs,
}

impl Pull {
	/// Run the destructive pull logic.
	pub async fn run(self, quiet: bool) -> Result<()> {
		let config = Config::load()?;
		let transfer = self.args.resolve_pull(&config)?;
		let client = connect(&config, quiet).await?;

		// Mark the directory as in use before remote work starts so background cleanup
		// does not treat an active old project as stale.
		record_connection_use(&config, &transfer.remote_dir);

		pull_project(
			&client,
			&config,
			&transfer.local_root,
			&transfer.remote_dir,
			&transfer.options,
			None,
			quiet,
		)
		.await?;

		record_connection_use(&config, &transfer.remote_dir);

		if config.clean.auto
			&& let Err(error) = spawn_background_cleanup(&config)
		{
			warn!(%error, "Failed to spawn background cleanup");
		}

		Ok(())
	}
}
