use crate::Result;
use crate::cache;
use crate::cli::clean::spawn_background_cleanup;
use crate::config::types::Config;
use crate::ssh::exec::connect;
use crate::ssh::sync::{Options, compute_project_remote_dir, sync_project};
use clap::Args;
use std::env;
use std::fs::canonicalize;
use std::path::PathBuf;
use tracing::warn;

/// Arguments for synchronization.
#[derive(Args, Debug, Default, Clone)]
pub struct SyncArgs {
	/// Base directory to start the synchronization from. Overrides the current working directory.
	#[arg(long)]
	pub sync_root: Option<PathBuf>,

	/// Override the remote project directory path. Bypasses the default `remote_root` + project name.
	#[arg(long, short = 'd')]
	pub remote_dir: Option<String>,

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
		let root = self
			.sync_root
			.clone()
			.or_else(|| config.sync.sync_root.clone())
			.map_or_else(env::current_dir, Ok)?;
		Ok(canonicalize(&root).unwrap_or(root))
	}

	/// Resolve the sync options.
	pub fn resolve_options(&self) -> Options {
		let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
		let cwd = canonicalize(&cwd).unwrap_or(cwd);
		let cwd_str = cwd.to_string_lossy().into_owned();
		let cwd_str = cwd_str.trim_end_matches('/');

		let make_absolute = |p: &String| {
			if p.starts_with('/') {
				p.clone()
			} else {
				format!("{cwd_str}/{}", p.trim_start_matches('/'))
			}
		};
		let exclude = self.exclude.iter().map(make_absolute).collect::<Vec<_>>();
		let include = self.include.iter().map(make_absolute).collect::<Vec<_>>();

		Options {
			force: self.force,
			exclude,
			include,
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
	pub async fn run(self, quiet: bool) -> Result<()> {
		let config = Config::load()?;
		let sync_root = self.sync_args.resolve_sync_root(&config)?;
		let options = self.sync_args.resolve_options();
		let client = connect(&config, quiet).await?;
		sync_project(
			&client,
			&config,
			&sync_root,
			&options,
			self.sync_args.remote_dir.as_deref(),
			quiet,
		)
		.await?;

		// Record the connection in the local cache.
		let remote_dir = self.sync_args.remote_dir.as_deref().map_or_else(
			|| compute_project_remote_dir(&config, &sync_root),
			|d| Ok(d.to_owned()),
		)?;
		if let Err(e) = cache::record_connection(
			&config.ssh.host,
			&config.ssh.user,
			config.ssh.port,
			&remote_dir,
		) {
			warn!(error = %e, "Failed to record connection in cache");
		}

		// Spawn background cleanup daemon if enabled.
		if config.clean.auto
			&& let Err(e) = spawn_background_cleanup() {
				warn!(error = %e, "Failed to spawn background cleanup");
			}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn resolve_options_absolute_paths() {
		let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
		let cwd = canonicalize(&cwd).unwrap_or(cwd);
		let cwd_str = cwd.to_string_lossy().into_owned();
		let cwd_str = cwd_str.trim_end_matches('/');

		let args = SyncArgs {
			exclude: vec!["/abs/exclude".to_owned(), "rel/exclude".to_owned()],
			include: vec!["/abs/include".to_owned(), "rel/include".to_owned()],
			..Default::default()
		};

		let options = args.resolve_options();

		assert_eq!(
			options.exclude,
			vec!["/abs/exclude".to_owned(), format!("{cwd_str}/rel/exclude")]
		);
		assert_eq!(
			options.include,
			vec!["/abs/include".to_owned(), format!("{cwd_str}/rel/include")]
		);
	}
}
