use crate::Result;
use crate::cli::clean::spawn_background_cleanup;
use crate::cli::transfer::{TransferArgs, record_connection_use};
use crate::cli::usage::CommandEffects;
use crate::config::types::Config;
use crate::ssh::exec::connect;
use crate::ssh::sync::push_project;
use clap::Args;
use tracing::warn;

/// Push local project files to the remote server.
#[derive(Args, Debug)]
#[clap(visible_alias = "s", visible_alias = "push")]
pub(super) struct Sync {
	/// Project transfer options.
	#[clap(flatten)]
	args: TransferArgs,
}

impl CommandEffects for Sync {
	const EFFECT: ::usage::SpecCommandEffect = ::usage::SpecCommandEffect::Destructive;
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
