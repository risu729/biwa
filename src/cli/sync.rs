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

	pub fn resolve_options(&self) -> Result<Options> {
		let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
		let cwd_str = cwd.display().to_string().replace('\\', "/");
		let cwd_str = cwd_str.trim_end_matches('/');

		let mut exclude = self.exclude.clone();
		for e in &mut exclude {
			if !e.starts_with('/') {
				*e = format!("{cwd_str}/{}", e.trim_start_matches('/'));
			}
		}

		let mut include = self.include.clone();
		for i in &mut include {
			if !i.starts_with('/') {
				*i = format!("{cwd_str}/{}", i.trim_start_matches('/'));
			}
		}

		Ok(Options {
			force: self.force,
			exclude,
			include,
		})
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
	pub async fn run(self, config: &Config, quiet: bool) -> Result<()> {
		let sync_root = self.sync_args.resolve_sync_root(config)?;
		let options = self.sync_args.resolve_options()?;
		sync_project(config, &sync_root, &options, quiet).await?;
		Ok(())
	}
}
