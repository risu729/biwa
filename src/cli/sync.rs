use crate::Result;
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

impl SyncArgs {
	/// Resolve the sync root directory.
	///
	/// Priority: CLI flag > config file > current working directory.
	pub fn resolve_sync_root(&self, config: &Config) -> Result<PathBuf> {
		Ok(self
			.sync_root
			.clone()
			.or_else(|| config.sync.sync_root.clone())
			.map_or_else(env::current_dir, Ok)?)
	}
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
#[clap(visible_alias = "s")]
pub(super) struct Sync {
	/// Synchronization options.
	#[clap(flatten)]
	sync_args: SyncArgs,
}

impl Sync {
	/// Run the sync logic.
	pub async fn run(self, config: &Config, quiet: bool, _silent: bool) -> Result<()> {
		let sync_root = self.sync_args.resolve_sync_root(config)?;
		sync_project(config, &sync_root, &self.sync_args.into(), quiet).await?;
		Ok(())
	}
}
