use crate::config::types::Config;
use crate::ssh::sync::{Options, sync_project};
use clap::Args;
use std::env;
use std::path::PathBuf;

/// Arguments for synchronization.
#[derive(Args, Debug, Default, Clone)]
pub struct SyncArgs {
	/// Base directory to start the synchronization from. Overrides the current working directory.
	#[arg(long)]
	pub sync_root: Option<PathBuf>,

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

impl From<SyncArgs> for Options {
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
- Secure Permissions: Enforces `0700` for directories. File permissions are preserved from the local filesystem but restricted to user-only access (e.g. `0644` becomes `0600`, `0755` becomes `0700`)."
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
		let sync_root = self
			.sync_args
			.sync_root
			.clone()
			.or_else(|| config.sync.sync_root.clone())
			.unwrap_or(current_dir);
		sync_project(config, &sync_root, &self.sync_args.into(), quiet).await?;
		Ok(())
	}
}
