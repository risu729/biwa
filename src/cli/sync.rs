use crate::config::types::Config;
use crate::ssh::sync::{SyncOptions, sync_project};
use clap::Args;
use std::env;

/// Arguments for synchronization.
#[derive(Args, Debug, Default, Clone)]
pub struct SyncArgs {
	/// Force synchronization of all files, ignoring incremental hash checks.
	#[arg(long, short = 'f')]
	pub force: bool,

	/// Exclude files matching these paths or globs.
	#[arg(long, short = 'e')]
	pub exclude: Vec<String>,

	/// Only synchronize files matching these paths or globs.
	#[arg(long, short = 'i')]
	pub include: Vec<String>,
}

impl From<SyncArgs> for SyncOptions {
	fn from(val: SyncArgs) -> Self {
		Self {
			force: val.force,
			exclude: val.exclude,
			include: val.include,
		}
	}
}

/// Synchronize local project files to the remote server.
#[derive(Args, Debug)]
#[clap(
	visible_alias = "s",
	long_about = "Synchronize local project files to the remote server.\n\n\
By default, `biwa run` automatically runs `biwa sync` before executing your command unless `sync.auto` is set to `false` in your configuration.\n\n\
Features:\n\
- Smart Hashing: Computes SHA-256 hash to only upload modified/new files.\n\
- Cleanup: Automatically deletes remote files that no longer exist locally.\n\
- Gitignore Support: Respects `.gitignore` and `.ignore` files automatically.\n\
- Secure Permissions: Enforces `0700` for directories and `0600` for files."
)]
pub(super) struct Sync {
	/// Synchronization options.
	#[clap(flatten)]
	sync_args: SyncArgs,
}

impl Sync {
	/// Run the sync logic.
	pub async fn run(self, config: &Config, quiet: bool, _silent: bool) -> eyre::Result<()> {
		let current_dir = env::current_dir()?;
		// Assuming current directory is project root or we can just use current directory.
		sync_project(config, &current_dir, &self.sync_args.into(), quiet).await?;
		Ok(())
	}
}
