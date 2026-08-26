use crate::Result;
use crate::cli::clean::spawn_background_cleanup;
use crate::cli::transfer::{TransferArgs, record_connection_use};
use crate::config::types::Config;
use crate::ssh::exec::connect;
use crate::ssh::sync::push_project;
use tracing::warn;

/// Push local project files to the remote host.
#[derive(usage_rs::Args, Debug)]
#[usage(effect = "destructive")]
pub(super) struct Sync {
	/// Project transfer options.
	#[usage(flatten)]
	args: TransferArgs,
}

impl Sync {
	/// Run the push logic.
	pub async fn run(self, quiet: bool) -> Result<()> {
		let config = Config::load()?;
		let transfer = self.args.resolve(&config)?;
		let client = connect(&config, quiet).await?;

		// Mark the directory as in use before remote work starts so background cleanup
		// does not treat an active old project as stale.
		record_connection_use(&config, &transfer.remote_dir);

		push_project(
			&client,
			&config,
			&transfer.local_root,
			&transfer.remote_dir,
			&transfer.options,
			quiet,
		)
		.await?;

		record_connection_use(&config, &transfer.remote_dir);

		if config.clean.auto
			&& let Err(error) =
				spawn_background_cleanup(&config, client.authentication_reusable_noninteractively())
		{
			warn!(%error, "Failed to spawn background cleanup");
		}

		Ok(())
	}
}
