use crate::Result;
use crate::config::types::Config;
use crate::ssh::sync::{Options, compute_project_remote_dir};
use crate::state;
use clap::Args;
use std::env;
#[cfg(unix)]
use std::fs::{Permissions, set_permissions};
use std::fs::{canonicalize, create_dir_all};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf, absolute};
use tracing::warn;

/// Direction-neutral project transfer arguments.
#[derive(Args, Debug, Default, Clone)]
pub(super) struct TransferArgs {
	/// Local project root used for synchronization.
	#[arg(long)]
	pub sync_root: Option<PathBuf>,

	/// Use the current working directory as the local project root instead of the nearest Git root.
	#[arg(long)]
	pub sync_cwd: bool,

	/// Override the remote project directory path. Bypasses the default `remote_root` + project name.
	#[arg(long, short = 'd')]
	pub remote_dir: Option<String>,

	/// Transfer selected files even when content hashes match.
	#[arg(long, short = 'f')]
	pub force: bool,

	/// Exclude files matching these paths or globs.
	#[arg(long, short = 'e')]
	pub exclude: Vec<String>,

	/// Only transfer files matching these paths or globs.
	#[arg(long, short = 'i')]
	pub include: Vec<String>,
}

/// A fully resolved local and remote transfer target.
pub(super) struct ResolvedTransfer {
	/// Local project root.
	pub local_root: PathBuf,
	/// Remote project directory.
	pub remote_dir: String,
	/// Direction-neutral transfer options.
	pub options: Options,
}

impl TransferArgs {
	/// Resolve the target for a push or round-trip operation.
	pub fn resolve(&self, config: &Config) -> Result<ResolvedTransfer> {
		let local_root = self.resolve_sync_root(config)?;
		let remote_dir = self.resolve_remote_dir(config, &local_root)?;
		Ok(ResolvedTransfer {
			local_root,
			remote_dir,
			options: self.resolve_options(),
		})
	}

	/// Resolve the target for a pull, creating a missing root only when its path is
	/// needed to derive the default remote project directory.
	pub fn resolve_pull(&self, config: &Config) -> Result<ResolvedTransfer> {
		let local_root = self.resolve_sync_root(config)?;
		if self.remote_dir.is_none() && !local_root.exists() {
			create_dir_all(&local_root)?;
			#[cfg(unix)]
			set_permissions(&local_root, Permissions::from_mode(0o700))?;
		}
		let remote_dir = self.resolve_remote_dir(config, &local_root)?;
		Ok(ResolvedTransfer {
			local_root,
			remote_dir,
			options: self.resolve_options(),
		})
	}

	/// Resolve the local synchronization root.
	pub(super) fn resolve_sync_root(&self, config: &Config) -> Result<PathBuf> {
		let default_to_git_root = self.default_to_git_root(config);
		self.resolve_sync_root_with(config, || default_sync_root(default_to_git_root))
	}

	/// Resolve the sync root directory using a supplied implicit default.
	fn resolve_sync_root_with(
		&self,
		config: &Config,
		default_root: impl FnOnce() -> io::Result<PathBuf>,
	) -> Result<PathBuf> {
		let root = self
			.sync_root
			.clone()
			.or_else(|| config.sync.sync_root.clone())
			.map_or_else(default_root, Ok)?;
		let absolute = absolute(&root)?;
		Ok(canonicalize(&absolute).unwrap_or(absolute))
	}

	/// Returns whether the implicit sync root should prefer the nearest Git root.
	const fn default_to_git_root(&self, config: &Config) -> bool {
		config.sync.default_to_git_root && !self.sync_cwd
	}

	/// Resolve the remote directory used for this transfer.
	fn resolve_remote_dir(&self, config: &Config, sync_root: &Path) -> Result<String> {
		self.remote_dir.as_deref().map_or_else(
			|| compute_project_remote_dir(config, sync_root),
			|dir| Ok(dir.to_owned()),
		)
	}

	/// Resolve direction-neutral transfer options.
	fn resolve_options(&self) -> Options {
		let cwd = canonical_current_dir();

		Options {
			force: self.force,
			exclude: absolutize_patterns(&self.exclude, &cwd),
			include: absolutize_patterns(&self.include, &cwd),
		}
	}
}

/// Records a remote directory use in local persisted state.
pub(super) fn record_connection_use(config: &Config, remote_dir: &str) {
	let state_dir = config.resolved_state_dir();
	if let Err(error) = state::record_connection(
		&state_dir,
		&config.ssh.host,
		&config.ssh.user,
		config.ssh.port,
		remote_dir,
	) {
		warn!(%error, "Failed to record connection in local state");
	}
}

/// Returns the default sync root for the current directory.
fn default_sync_root(default_to_git_root: bool) -> io::Result<PathBuf> {
	let cwd = env::current_dir()?;
	Ok(default_sync_root_from(&cwd, default_to_git_root))
}

/// Returns the default sync root for `cwd`.
fn default_sync_root_from(cwd: &Path, default_to_git_root: bool) -> PathBuf {
	if default_to_git_root {
		find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
	} else {
		cwd.to_path_buf()
	}
}

/// Finds the nearest Git worktree root at or above `start`.
fn find_git_root(start: &Path) -> Option<PathBuf> {
	start
		.ancestors()
		.find(|path| path.join(".git").exists())
		.map(Path::to_path_buf)
}

/// Returns the canonical current directory, falling back to `.` if needed.
fn canonical_current_dir() -> PathBuf {
	let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
	canonicalize(&cwd).unwrap_or(cwd)
}

/// Converts relative include/exclude patterns into absolute paths.
fn absolutize_patterns(patterns: &[String], base_dir: &Path) -> Vec<String> {
	let base_dir = trim_trailing_slash(base_dir);
	patterns
		.iter()
		.map(|pattern| absolutize_pattern(pattern, &base_dir))
		.collect()
}

/// Converts one transfer pattern into an absolute path if it is relative.
fn absolutize_pattern(pattern: &str, base_dir: &str) -> String {
	if pattern.starts_with('/') {
		pattern.to_owned()
	} else {
		format!("{base_dir}/{}", pattern.trim_start_matches('/'))
	}
}

/// Returns a displayable path without a trailing slash.
fn trim_trailing_slash(path: &Path) -> String {
	path.to_string_lossy().trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use std::fs;
	use tempfile::tempdir;

	#[test]
	fn resolve_sync_root_uses_git_root_from_subdirectory() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path().join("project");
		let nested = root.join("src/bin");
		fs::create_dir_all(&nested)?;
		fs::create_dir_all(root.join(".git"))?;

		let root = canonicalize(&root).unwrap_or(root);
		let args = TransferArgs::default();

		assert_eq!(
			args.resolve_sync_root_with(&Config::default(), || {
				Ok(default_sync_root_from(
					&nested,
					args.default_to_git_root(&Config::default()),
				))
			})?,
			root
		);
		Ok(())
	}

	#[test]
	fn resolve_sync_root_uses_current_directory_without_git_root() -> Result<()> {
		let dir = tempdir()?;
		let nested = dir.path().join("standalone/nested");
		fs::create_dir_all(&nested)?;

		let nested = canonicalize(&nested).unwrap_or(nested);
		let args = TransferArgs::default();

		assert_eq!(
			args.resolve_sync_root_with(&Config::default(), || {
				Ok(default_sync_root_from(
					&nested,
					args.default_to_git_root(&Config::default()),
				))
			})?,
			nested
		);
		Ok(())
	}

	#[test]
	fn resolve_sync_root_uses_current_directory_when_config_disables_git_root() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path().join("project");
		let nested = root.join("src");
		fs::create_dir_all(&nested)?;
		fs::create_dir_all(root.join(".git"))?;

		let nested = canonicalize(&nested).unwrap_or(nested);
		let mut config = Config::default();
		config.sync.default_to_git_root = false;
		let args = TransferArgs::default();

		assert_eq!(
			args.resolve_sync_root_with(&config, || Ok(default_sync_root_from(
				&nested,
				args.default_to_git_root(&config),
			)))?,
			nested
		);
		Ok(())
	}

	#[test]
	fn resolve_sync_root_uses_current_directory_when_cli_disables_git_root() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path().join("project");
		let nested = root.join("src");
		fs::create_dir_all(&nested)?;
		fs::create_dir_all(root.join(".git"))?;

		let nested = canonicalize(&nested).unwrap_or(nested);
		let config = Config::default();
		let args = TransferArgs {
			sync_cwd: true,
			..Default::default()
		};

		assert_eq!(
			args.resolve_sync_root_with(&config, || Ok(default_sync_root_from(
				&nested,
				args.default_to_git_root(&config),
			)))?,
			nested
		);
		Ok(())
	}

	#[test]
	fn resolve_sync_root_preserves_explicit_config_root() -> Result<()> {
		let dir = tempdir()?;
		let configured_root = dir.path().join("configured");
		fs::create_dir_all(&configured_root)?;

		let mut config = Config::default();
		config.sync.sync_root = Some(configured_root.clone());
		let args = TransferArgs::default();

		assert_eq!(
			args.resolve_sync_root_with(&config, default_root_should_not_be_used)?,
			canonicalize(&configured_root).unwrap_or(configured_root)
		);
		Ok(())
	}

	#[test]
	fn resolve_sync_root_preserves_explicit_cli_root() -> Result<()> {
		let dir = tempdir()?;
		let configured_root = dir.path().join("configured");
		let cli_root = dir.path().join("cli");
		fs::create_dir_all(&configured_root)?;
		fs::create_dir_all(&cli_root)?;

		let mut config = Config::default();
		config.sync.sync_root = Some(configured_root);
		let args = TransferArgs {
			sync_root: Some(cli_root.clone()),
			..Default::default()
		};

		assert_eq!(
			args.resolve_sync_root_with(&config, default_root_should_not_be_used)?,
			canonicalize(&cli_root).unwrap_or(cli_root)
		);
		Ok(())
	}

	#[test]
	fn find_git_root_accepts_worktree_git_file() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path().join("project");
		let nested = root.join("src");
		fs::create_dir_all(&nested)?;
		fs::write(root.join(".git"), "gitdir: ../.git/worktrees/project\n")?;

		assert_eq!(find_git_root(&nested), Some(root));
		Ok(())
	}

	fn default_root_should_not_be_used() -> io::Result<PathBuf> {
		Err(io::Error::other("explicit sync root should skip default"))
	}

	#[test]
	fn resolve_options_absolute_paths() {
		let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
		let cwd = canonicalize(&cwd).unwrap_or(cwd);
		let cwd_str = cwd.to_string_lossy().into_owned();
		let cwd_str = cwd_str.trim_end_matches('/');

		let args = TransferArgs {
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

	#[test]
	fn absolutize_patterns_preserves_absolute_entries() {
		assert_eq!(
			absolutize_patterns(
				&["/abs/path".to_owned(), "relative/path".to_owned()],
				Path::new("/project"),
			),
			vec!["/abs/path".to_owned(), "/project/relative/path".to_owned()]
		);
	}
}
