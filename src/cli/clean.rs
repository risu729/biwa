use crate::Result;
use crate::cli::sync::SyncArgs;
use crate::config::types::{Config, PasswordConfig};
use crate::duration::HumanDuration;
use crate::ssh::clean::{
	QuotaUsage, RemoteDirEntry, check_quota, list_remote_dir_entries, list_remote_dirs,
	remove_remote_dir,
};
use crate::ssh::client::Client;
use crate::ssh::exec::connect;
use crate::ssh::sync::{
	compute_client_host_hash, compute_project_remote_dir, is_biwa_remote_dir,
	is_default_biwa_remote_dir,
};
use crate::state::{
	Connection, State, default_state_dir, is_daemon_running, kill_daemon, load_state,
	remove_connections_for_target, remove_pid_file, stale_connections, write_pid_file,
};
use alloc::sync::Arc;
use chrono::{DateTime, Utc};
use clap::{Args, ValueEnum};
use color_eyre::eyre::{Context as _, bail};
use console::style;
use core::time::Duration;
use nix::unistd;
use std::collections::HashSet;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

/// Clean stale remote project directories.
#[derive(Args, Debug)]
#[command(visible_alias = "c")]
#[expect(
	clippy::struct_excessive_bools,
	reason = "Each bool maps to an independent CLI flag with distinct semantics"
)]
pub(super) struct Clean {
	/// Optional clean action (`stop` stops the background cleanup daemon).
	#[arg(value_enum)]
	action: Option<CleanAction>,

	/// Remove all this client's tracked remote directories.
	#[arg(long)]
	all: bool,

	/// Remove ALL biwa directories under `remote_root` (including other clients).
	#[arg(long)]
	purge: bool,

	/// Preview what would be removed without deleting.
	#[arg(long)]
	dry_run: bool,

	/// Background auto-cleanup mode (used internally by the daemon).
	#[arg(long, hide = true)]
	auto: bool,
}

/// Optional action for `biwa clean`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum CleanAction {
	/// Stop the running background cleanup daemon.
	Stop,
}

/// Which cleanup mode `biwa clean` runs (after handling [`CleanAction::Stop`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CleanTarget {
	/// Default: remove only the current project’s remote directory.
	CurrentProject,
	/// `--auto`: quota / stale connection cleanup (daemon-style).
	Auto,
	/// `--all`: remove all tracked default-layout remote dirs for this client.
	All,
	/// `--purge`: remove every directory under `remote_root` on the server.
	Purge,
}

/// Maps mutually exclusive `clean` flags to a [`CleanTarget`].
///
/// Precedence: `--auto`, then `--purge`, then `--all`, then default (current project).
#[must_use]
pub(super) const fn clean_target(auto: bool, purge: bool, all: bool) -> CleanTarget {
	if auto {
		CleanTarget::Auto
	} else if purge {
		CleanTarget::Purge
	} else if all {
		CleanTarget::All
	} else {
		CleanTarget::CurrentProject
	}
}

impl Clean {
	/// Run the clean command.
	pub async fn run(self, quiet: bool) -> Result<()> {
		// Handle `biwa clean stop`.
		if matches!(self.action, Some(CleanAction::Stop)) {
			stop_daemon(quiet);
			return Ok(());
		}

		let config = Config::load()?;
		let state_dir = config.resolved_state_dir();

		let target = clean_target(self.auto, self.purge, self.all);
		if matches!(target, CleanTarget::Auto) {
			return run_auto_cleanup(&config, &state_dir).await;
		}

		// For destructive explicit clean commands, kill any running daemon first.
		if !self.dry_run && is_daemon_running(&state_dir) {
			kill_daemon(&state_dir);
			if !quiet {
				eprintln!(
					"{} Stopped background cleanup daemon",
					style("✓").green().bold()
				);
			}
		}

		if matches!(target, CleanTarget::Purge) {
			return run_purge_cleanup(&config, self.dry_run, quiet).await;
		}
		if matches!(target, CleanTarget::All) {
			return run_all_cleanup(&config, self.dry_run, quiet).await;
		}
		run_current_cleanup(&config, self.dry_run, quiet).await
	}
}

/// Stop the background cleanup daemon.
fn stop_daemon(quiet: bool) {
	let state_dir = Config::load().map_or_else(
		|_| state_dir_from_env_or_default(),
		|config| config.resolved_state_dir(),
	);
	if is_daemon_running(&state_dir) {
		kill_daemon(&state_dir);
		if !quiet {
			eprintln!(
				"{} Stopped background cleanup daemon",
				style("✓").green().bold()
			);
		}
	} else {
		if !quiet {
			eprintln!("No background cleanup daemon is running");
		}
		remove_pid_file(&state_dir);
	}
}

/// Resolves state directory without requiring a valid config file.
fn state_dir_from_env_or_default() -> PathBuf {
	env::var_os("BIWA_STATE_DIR").map_or_else(default_state_dir, PathBuf::from)
}

/// Upper bound on concurrent SSH sessions used for bulk `rm -rf`.
const MAX_CONCURRENT_REMOTE_REMOVALS: usize = 3;

/// Removes multiple remote directories with bounded parallelism.
async fn remove_remote_dirs_bounded(
	client: &Client,
	paths: &[String],
	failed_task_log: &'static str,
) -> Result<(Vec<String>, usize)> {
	let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_REMOTE_REMOVALS));
	let mut join_set = JoinSet::new();
	for path in paths {
		let permit = Arc::clone(&semaphore)
			.acquire_owned()
			.await
			.wrap_err("Failed to acquire cleanup semaphore")?;
		let client_clone = client.clone();
		let path_clone = path.clone();
		join_set.spawn(async move {
			let _permit = permit;
			let result = remove_remote_dir(&client_clone, &path_clone).await;
			(path_clone, result)
		});
	}

	let mut errors = 0_usize;
	let mut succeeded: Vec<String> = Vec::new();
	while let Some(result) = join_set.join_next().await {
		match result {
			Ok((path, Ok(()))) => succeeded.push(path),
			Ok((_, Err(e))) => {
				warn!(error = %e, "{}", failed_task_log);
				errors = errors.saturating_add(1);
			}
			Err(e) => {
				warn!(error = %e, "Task panicked during remote directory removal");
				errors = errors.saturating_add(1);
			}
		}
	}

	Ok((succeeded, errors))
}

/// Clean current project's remote directory.
async fn run_current_cleanup(config: &Config, dry_run: bool, quiet: bool) -> Result<()> {
	let sync_root = resolve_current_project_root(config)?;
	let remote_dir = compute_project_remote_dir(config, &sync_root)?;

	if dry_run {
		eprintln!("Would remove: {remote_dir}");
		return Ok(());
	}

	let client = connect(config, quiet).await?;
	remove_remote_dir(&client, &remote_dir).await?;
	let state_dir = config.resolved_state_dir();
	remove_connections_for_target(
		&state_dir,
		&config.ssh.host,
		&config.ssh.user,
		config.ssh.port,
		&[remote_dir.as_str()],
	)?;

	if !quiet {
		eprintln!(
			"{} Removed remote directory: {remote_dir}",
			style("✓").green().bold()
		);
	}
	Ok(())
}

/// Resolves the project root used by the default clean target.
fn resolve_current_project_root(config: &Config) -> Result<PathBuf> {
	SyncArgs::default().resolve_sync_root(config)
}

/// Clean all this client's tracked remote directories.
async fn run_all_cleanup(config: &Config, dry_run: bool, quiet: bool) -> Result<()> {
	let state_dir = config.resolved_state_dir();
	let state = load_state(&state_dir)?;
	let host_hash = compute_client_host_hash();
	let remote_root = &config.sync.remote_root;

	// Filter to connections matching current SSH config and biwa's default remote layout.
	let matching: Vec<_> = state
		.connections
		.iter()
		.filter(|c| {
			c.host == config.ssh.host && c.user == config.ssh.user && c.port == config.ssh.port
		})
		.filter(|c| is_default_biwa_remote_dir(&c.remote_dir, remote_root, &host_hash))
		.collect();

	if matching.is_empty() {
		if !quiet {
			eprintln!("No tracked remote directories to clean");
		}
		return Ok(());
	}

	if dry_run {
		for conn in &matching {
			eprintln!("Would remove: {}", conn.remote_dir);
		}
		return Ok(());
	}

	let client = connect(config, quiet).await?;

	let dirs_to_remove: Vec<String> = matching.iter().map(|c| c.remote_dir.clone()).collect();
	let (succeeded, errors) = remove_remote_dirs_bounded(
		&client,
		&dirs_to_remove,
		"Failed to remove a remote directory",
	)
	.await?;

	// Only remove successfully deleted entries from persisted state.
	let dir_refs: Vec<&str> = succeeded.iter().map(String::as_str).collect();
	remove_connections_for_target(
		&state_dir,
		&config.ssh.host,
		&config.ssh.user,
		config.ssh.port,
		&dir_refs,
	)?;

	if !quiet {
		eprintln!(
			"{} Cleaned {} remote directories ({} errors)",
			style("✓").green().bold(),
			succeeded.len(),
			errors
		);
	}

	if errors > 0 {
		bail!("Some directories could not be removed");
	}

	Ok(())
}

/// Clean ALL biwa directories under `remote_root` (including other clients).
async fn run_purge_cleanup(config: &Config, dry_run: bool, quiet: bool) -> Result<()> {
	let client = connect(config, quiet).await?;
	let remote_root = config.sync.remote_root.to_string_lossy().into_owned();
	let dirs = list_remote_dirs(&client, &remote_root).await?;
	let full_paths = purge_cleanup_paths(&config.sync.remote_root, &dirs);

	if full_paths.is_empty() {
		if !quiet {
			eprintln!("No biwa directories found under {remote_root}");
		}
		return Ok(());
	}

	if dry_run {
		for path in &full_paths {
			eprintln!("Would remove: {path}");
		}
		return Ok(());
	}

	let (succeeded, errors) =
		remove_remote_dirs_bounded(&client, &full_paths, "Failed to remove a remote directory")
			.await?;

	// Only remove successfully deleted entries from persisted state.
	let dir_refs: Vec<&str> = succeeded.iter().map(String::as_str).collect();
	let state_dir = config.resolved_state_dir();
	remove_connections_for_target(
		&state_dir,
		&config.ssh.host,
		&config.ssh.user,
		config.ssh.port,
		&dir_refs,
	)?;

	if !quiet {
		eprintln!(
			"{} Purged {} directories under {remote_root} ({} errors)",
			style("✓").green().bold(),
			succeeded.len(),
			errors
		);
	}

	if errors > 0 {
		bail!("Some directories could not be removed");
	}

	Ok(())
}

/// Returns purge candidates from direct child names listed under `remote_root`.
fn purge_cleanup_paths(remote_root: &Path, dirs: &[String]) -> Vec<String> {
	let remote_root_str = remote_root.to_string_lossy();
	let remote_root_str = remote_root_str.as_ref();
	dirs.iter()
		.map(|dir| join_remote_child(remote_root_str, dir))
		.filter(|path| is_biwa_remote_dir(path, remote_root))
		.collect()
}

/// Automatic background cleanup driven by quota thresholds.
async fn run_auto_cleanup(config: &Config, state_dir: &Path) -> Result<()> {
	// Ensure only one daemon runs at a time.
	let already_running = write_pid_file(state_dir)?;
	if already_running {
		debug!("Another cleanup daemon is already running; exiting");
		return Ok(());
	}

	// Clean up PID file when we're done, regardless of success or failure.
	let _pid_guard = scopeguard::guard((), |()| remove_pid_file(state_dir));

	let state = load_state(state_dir)?;
	let host_hash = compute_client_host_hash();
	let remote_root = &config.sync.remote_root;

	let client = connect(config, true).await?;

	// Check quota to determine which threshold to apply.
	let quota = check_quota(&client).await?;
	let usage_percent = quota.as_ref().map_or(0.0, QuotaUsage::usage_percent);

	let Some(max_age) = applicable_cleanup_threshold(config, quota.is_some(), usage_percent) else {
		debug!("No applicable cleanup threshold");
		return Ok(());
	};

	let to_remove =
		auto_cleanup_candidates(&client, config, &state, max_age, remote_root, &host_hash).await?;

	if to_remove.is_empty() {
		debug!("No stale directories to clean");
		return Ok(());
	}

	let stale_dirs: Vec<String> = to_remove.into_iter().collect();

	info!(
		count = stale_dirs.len(),
		max_age_secs = max_age.as_secs(),
		"Cleaning stale remote directories"
	);

	let (succeeded, errors) =
		remove_remote_dirs_bounded(&client, &stale_dirs, "Failed to remove a stale directory")
			.await?;

	// Only remove successfully deleted entries from persisted state.
	let dir_refs: Vec<&str> = succeeded.iter().map(String::as_str).collect();
	remove_connections_for_target(
		state_dir,
		&config.ssh.host,
		&config.ssh.user,
		config.ssh.port,
		&dir_refs,
	)?;

	info!(
		removed = succeeded.len(),
		errors, "Background cleanup completed"
	);

	Ok(())
}

/// Selects the cleanup age limit for the current quota state.
fn applicable_cleanup_threshold(
	config: &Config,
	has_quota: bool,
	usage_percent: f64,
) -> Option<Duration> {
	let thresholds = config.clean.effective_thresholds();
	info!(
		usage_percent = format!("{usage_percent:.1}"),
		threshold_count = thresholds.len(),
		"Checking cleanup thresholds"
	);

	if has_quota {
		return thresholds
			.iter()
			.rev()
			.find(|(pct, _)| usage_percent >= f64::from(**pct))
			.map(|(_, duration)| duration.as_duration());
	}

	// No quota available: only apply the 0% threshold (max_age).
	thresholds.get(&0).map(HumanDuration::as_duration)
}

/// Collects tracked stale dirs and safe orphan dirs for automatic cleanup.
async fn auto_cleanup_candidates(
	client: &Client,
	config: &Config,
	state: &State,
	max_age: Duration,
	remote_root: &Path,
	host_hash: &str,
) -> Result<HashSet<String>> {
	let tracked_default_dirs = collect_tracked_default_dirs(config, state, remote_root, host_hash);
	let mut to_remove: HashSet<String> = stale_connections(state, max_age)
		.into_iter()
		.filter(|connection| default_target_connection(config, connection, remote_root, host_hash))
		.map(|connection| connection.remote_dir.clone())
		.collect();

	// Only treat "orphan" remote dirs (present on server but not in local state) as candidates
	// when we have at least one tracked default-layout path for this target. If local state has
	// only custom remote dirs, empty state, or broken state, orphan detection is unsafe.
	if tracked_default_dirs.is_empty() {
		return Ok(to_remove);
	}

	let remote_root_str = remote_root.to_string_lossy().into_owned();
	let now = Utc::now();
	let listed = list_remote_dir_entries(client, &remote_root_str).await?;
	for entry in &listed {
		let full_path = join_remote_child(&remote_root_str, &entry.name);
		if is_default_biwa_remote_dir(&full_path, remote_root, host_hash)
			&& !tracked_default_dirs.contains(&full_path)
			&& remote_dir_is_older_than(entry, max_age, now)
		{
			to_remove.insert(full_path);
		}
	}

	Ok(to_remove)
}

/// Returns whether a remote directory mtime is older than the cleanup threshold.
fn remote_dir_is_older_than(
	entry: &RemoteDirEntry,
	threshold: Duration,
	now: DateTime<Utc>,
) -> bool {
	now.signed_duration_since(entry.modified_at)
		.to_std()
		.is_ok_and(|age| age > threshold)
}

/// Returns tracked default-layout dirs for the current SSH target.
fn collect_tracked_default_dirs(
	config: &Config,
	state: &State,
	remote_root: &Path,
	host_hash: &str,
) -> HashSet<String> {
	state
		.connections
		.iter()
		.filter(|connection| default_target_connection(config, connection, remote_root, host_hash))
		.map(|connection| connection.remote_dir.clone())
		.collect()
}

/// Returns whether a state connection belongs to this target and default biwa layout.
fn default_target_connection(
	config: &Config,
	connection: &Connection,
	remote_root: &Path,
	host_hash: &str,
) -> bool {
	connection.host == config.ssh.host
		&& connection.user == config.ssh.user
		&& connection.port == config.ssh.port
		&& is_default_biwa_remote_dir(&connection.remote_dir, remote_root, host_hash)
}

/// Joins a remote root and direct child name without introducing a duplicate slash.
fn join_remote_child(remote_root: &str, child: &str) -> String {
	if remote_root.ends_with('/') {
		format!("{remote_root}{child}")
	} else {
		format!("{remote_root}/{child}")
	}
}

/// Spawns a detached background process to run `biwa clean --auto`.
///
/// The child process is fully detached (new session, stdio to /dev/null) so it
/// survives the parent exiting.
pub fn spawn_background_cleanup(config: &Config) -> Result<()> {
	if matches!(config.ssh.password, PasswordConfig::Interactive(true)) {
		warn!(
			"Skipping background auto-cleanup: ssh.password is interactive-only; use a string password, SSH key, or agent authentication"
		);
		return Ok(());
	}

	let state_dir = config.resolved_state_dir();

	if is_daemon_running(&state_dir) {
		debug!("Background cleanup daemon is already running; skipping");
		return Ok(());
	}

	let exe = env::current_exe()?;
	let mut cmd = Command::new(exe);
	cmd.args(["clean", "--auto", "--quiet"]);
	configure_daemon_env(&mut cmd, config, &state_dir);

	cmd.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null());

	// Detach by creating a new session.
	#[cfg(unix)]
	{
		use nix::errno::Errno;
		use std::os::unix::process::CommandExt as _;
		// SAFETY: setsid() is safe to call pre-exec; it only affects the child process.
		unsafe {
			cmd.pre_exec(|| {
				unistd::setsid().map_err(|e: Errno| io::Error::from(e))?;
				Ok(())
			});
		}
	}

	cmd.spawn().map_err(|e| {
		debug!(error = %e, "Failed to spawn background cleanup process");
		e
	})?;

	debug!("Spawned background cleanup process");
	Ok(())
}

/// Applies resolved config values needed by the detached cleanup process.
fn configure_daemon_env(cmd: &mut Command, config: &Config, state_dir: &Path) {
	cmd.env("BIWA_SSH_HOST", &config.ssh.host);
	cmd.env("BIWA_SSH_PORT", config.ssh.port.to_string());
	cmd.env("BIWA_SSH_USER", &config.ssh.user);
	cmd.env("BIWA_SSH_UMASK", config.ssh.umask.to_string());
	cmd.env("BIWA_SYNC_REMOTE_ROOT", &config.sync.remote_root);
	cmd.env("BIWA_STATE_DIR", state_dir);

	if let Some(key_path) = &config.ssh.key_path {
		cmd.env("BIWA_SSH_KEY_PATH", key_path);
	}

	match &config.ssh.password {
		PasswordConfig::Value(password) => {
			cmd.env("BIWA_SSH_PASSWORD", password);
		}
		PasswordConfig::Interactive(false) => {
			cmd.env("BIWA_SSH_PASSWORD", "false");
		}
		PasswordConfig::Interactive(true) => {}
	}
}

#[cfg(test)]
mod tests {
	use super::{
		CleanTarget, clean_target, configure_daemon_env, join_remote_child, purge_cleanup_paths,
		remote_dir_is_older_than, resolve_current_project_root, state_dir_from_env_or_default,
	};
	use crate::config::types::{Config, PasswordConfig};
	use crate::ssh::clean::RemoteDirEntry;
	use crate::testing::EnvCleanup;
	use alloc::collections::BTreeMap;
	use chrono::Utc;
	use core::time::Duration;
	use pretty_assertions::assert_eq;
	use std::path::{Path, PathBuf};
	use std::process::Command;
	use std::{env, fs};

	struct CurrentDirGuard(PathBuf);

	impl CurrentDirGuard {
		fn set(path: &Path) -> Self {
			let previous = env::current_dir().expect("current dir is available");
			env::set_current_dir(path).expect("set current dir");
			Self(previous)
		}
	}

	impl Drop for CurrentDirGuard {
		fn drop(&mut self) {
			env::set_current_dir(&self.0).expect("restore current dir");
		}
	}

	#[test]
	fn clean_target_none_is_current_project() {
		assert_eq!(
			clean_target(false, false, false),
			CleanTarget::CurrentProject
		);
	}

	#[test]
	fn clean_target_auto() {
		assert_eq!(clean_target(true, false, false), CleanTarget::Auto);
		assert_eq!(clean_target(true, true, true), CleanTarget::Auto);
	}

	#[test]
	fn clean_target_all() {
		assert_eq!(clean_target(false, false, true), CleanTarget::All);
	}

	#[test]
	fn clean_target_purge() {
		assert_eq!(clean_target(false, true, false), CleanTarget::Purge);
	}

	#[test]
	fn clean_target_purge_wins_over_all() {
		assert_eq!(clean_target(false, true, true), CleanTarget::Purge);
	}

	#[test]
	fn join_remote_child_does_not_duplicate_separator() {
		assert_eq!(join_remote_child("~/root", "child"), "~/root/child");
		assert_eq!(join_remote_child("~/root/", "child"), "~/root/child");
	}

	#[test]
	fn purge_cleanup_paths_keep_only_biwa_layout_dirs() {
		let paths = purge_cleanup_paths(
			Path::new("~/root"),
			&[
				"project-a1b2c3d4-deadbeef".to_owned(),
				"legacy-deadbeef".to_owned(),
				"ordinary-directory".to_owned(),
				"nested/project-deadbeef".to_owned(),
			],
		);

		assert_eq!(
			paths,
			vec![
				"~/root/project-a1b2c3d4-deadbeef".to_owned(),
				"~/root/legacy-deadbeef".to_owned()
			]
		);
	}

	#[test]
	#[serial_test::serial]
	fn state_dir_from_env_or_default_respects_env_without_config() {
		let _cleanup = EnvCleanup::set("BIWA_STATE_DIR", "/tmp/biwa-state-test");

		assert_eq!(
			state_dir_from_env_or_default(),
			PathBuf::from("/tmp/biwa-state-test")
		);
	}

	#[test]
	fn remote_dir_mtime_must_exceed_threshold() {
		let now = Utc::now();
		let entry = RemoteDirEntry {
			name: "project-abcd1234-deadbeef".to_owned(),
			modified_at: now - chrono::Duration::days(31),
		};

		assert!(remote_dir_is_older_than(
			&entry,
			Duration::from_hours(30 * 24),
			now
		));
	}

	#[test]
	fn future_remote_dir_mtime_is_not_stale() {
		let now = Utc::now();
		let entry = RemoteDirEntry {
			name: "project-abcd1234-deadbeef".to_owned(),
			modified_at: now + chrono::Duration::minutes(1),
		};

		assert!(!remote_dir_is_older_than(
			&entry,
			Duration::from_secs(0),
			now
		));
	}

	#[test]
	fn resolve_current_project_root_prefers_config_sync_root() {
		let dir = tempfile::tempdir().unwrap();
		let mut config = Config::default();
		config.sync.sync_root = Some(dir.path().to_path_buf());

		assert_eq!(
			resolve_current_project_root(&config).unwrap(),
			dir.path().canonicalize().unwrap()
		);
	}

	#[test]
	#[serial_test::serial]
	fn resolve_current_project_root_uses_git_root_by_default() {
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path().join("project");
		let nested = root.join("src/bin");
		fs::create_dir_all(&nested).unwrap();
		fs::create_dir_all(root.join(".git")).unwrap();
		let _cwd = CurrentDirGuard::set(&nested);

		let config = Config::default();

		assert_eq!(
			resolve_current_project_root(&config).unwrap(),
			root.canonicalize().unwrap()
		);
	}

	#[test]
	fn configure_daemon_env_uses_resolved_config_values() {
		let mut config = Config::default();
		config.ssh.host = "example.test".to_owned();
		config.ssh.port = 2222;
		config.ssh.user = "alice".to_owned();
		config.ssh.key_path = Some(PathBuf::from("/tmp/key"));
		config.ssh.password = PasswordConfig::Value("secret".to_owned());
		config.sync.remote_root = PathBuf::from("~/remote");

		let mut cmd = Command::new("biwa");
		configure_daemon_env(&mut cmd, &config, Path::new("/tmp/state"));

		let envs = cmd
			.get_envs()
			.filter_map(|(key, value)| {
				value.map(|value| {
					(
						key.to_string_lossy().into_owned(),
						value.to_string_lossy().into_owned(),
					)
				})
			})
			.collect::<BTreeMap<_, _>>();

		assert_eq!(
			envs.get("BIWA_SSH_HOST").map(String::as_str),
			Some("example.test")
		);
		assert_eq!(envs.get("BIWA_SSH_PORT").map(String::as_str), Some("2222"));
		assert_eq!(envs.get("BIWA_SSH_USER").map(String::as_str), Some("alice"));
		assert_eq!(
			envs.get("BIWA_SSH_PASSWORD").map(String::as_str),
			Some("secret")
		);
		assert_eq!(
			envs.get("BIWA_SSH_KEY_PATH").map(String::as_str),
			Some("/tmp/key")
		);
		assert_eq!(
			envs.get("BIWA_SYNC_REMOTE_ROOT").map(String::as_str),
			Some("~/remote")
		);
		assert_eq!(
			envs.get("BIWA_STATE_DIR").map(String::as_str),
			Some("/tmp/state")
		);
	}
}
