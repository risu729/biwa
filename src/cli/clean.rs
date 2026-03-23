use crate::Result;
use crate::config::types::{Config, PasswordConfig};
use crate::duration::HumanDuration;
use crate::ssh::clean::{QuotaUsage, check_quota, list_remote_dirs, remove_remote_dir};
use crate::ssh::client::Client;
use crate::ssh::exec::connect;
use crate::ssh::sync::{
	compute_client_host_hash, compute_project_remote_dir, is_default_biwa_remote_dir,
};
use crate::state::{
	is_daemon_running, kill_daemon, load_state, remove_connections_for_target, remove_pid_file,
	stale_connections, write_pid_file,
};
use alloc::sync::Arc;
use clap::{Args, Subcommand};
use color_eyre::eyre::{Context as _, bail};
use console::style;
use nix::unistd;
use std::collections::HashSet;
use std::io;
use std::process::{Command, Stdio};
use std::{env, fs};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

/// Clean stale remote project directories.
#[derive(Args, Debug)]
#[command(visible_alias = "c", subcommand_required = false)]
#[expect(
	clippy::struct_excessive_bools,
	reason = "Each bool maps to an independent CLI flag with distinct semantics"
)]
pub(super) struct Clean {
	/// Subcommand (e.g. `stop`).
	#[command(subcommand)]
	subcommand: Option<CleanSubcommand>,

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

/// Subcommands for `biwa clean`.
#[derive(Subcommand, Debug)]
enum CleanSubcommand {
	/// Stop the running background cleanup daemon.
	Stop,
}

impl Clean {
	/// Run the clean command.
	pub async fn run(self, quiet: bool) -> Result<()> {
		// Handle `biwa clean stop` subcommand.
		if matches!(self.subcommand, Some(CleanSubcommand::Stop)) {
			stop_daemon(quiet);
			return Ok(());
		}

		let config = Config::load()?;

		if self.auto {
			return run_auto_cleanup(&config).await;
		}

		// For explicit clean commands, kill any running daemon first.
		if is_daemon_running() {
			kill_daemon();
			if !quiet {
				eprintln!(
					"{} Stopped background cleanup daemon",
					style("✓").green().bold()
				);
			}
		}

		if self.purge {
			return run_purge_cleanup(&config, self.dry_run, quiet).await;
		}

		if self.all {
			return run_all_cleanup(&config, self.dry_run, quiet).await;
		}

		run_current_cleanup(&config, self.dry_run, quiet).await
	}
}

/// Stop the background cleanup daemon.
fn stop_daemon(quiet: bool) {
	if is_daemon_running() {
		kill_daemon();
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
		remove_pid_file();
	}
}

/// Upper bound on concurrent SSH sessions used for bulk `rm -rf`.
const MAX_CONCURRENT_REMOTE_REMOVALS: usize = 8;

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
	let sync_root = env::current_dir()?;
	let sync_root = fs::canonicalize(&sync_root).unwrap_or(sync_root);
	let remote_dir = compute_project_remote_dir(config, &sync_root)?;

	if dry_run {
		eprintln!("Would remove: {remote_dir}");
		return Ok(());
	}

	let client = connect(config, quiet).await?;
	remove_remote_dir(&client, &remote_dir).await?;
	remove_connections_for_target(
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

/// Clean all this client's tracked remote directories.
async fn run_all_cleanup(config: &Config, dry_run: bool, quiet: bool) -> Result<()> {
	let state = load_state()?;
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

	if dirs.is_empty() {
		if !quiet {
			eprintln!("No directories found under {remote_root}");
		}
		return Ok(());
	}

	if dry_run {
		for dir in &dirs {
			eprintln!("Would remove: {remote_root}/{dir}");
		}
		return Ok(());
	}

	let full_paths: Vec<String> = dirs.iter().map(|d| format!("{remote_root}/{d}")).collect();
	let (succeeded, errors) =
		remove_remote_dirs_bounded(&client, &full_paths, "Failed to remove a remote directory")
			.await?;

	// Only remove successfully deleted entries from persisted state.
	let dir_refs: Vec<&str> = succeeded.iter().map(String::as_str).collect();
	remove_connections_for_target(
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

/// Automatic background cleanup driven by quota thresholds.
async fn run_auto_cleanup(config: &Config) -> Result<()> {
	// Ensure only one daemon runs at a time.
	let already_running = write_pid_file()?;
	if already_running {
		debug!("Another cleanup daemon is already running; exiting");
		return Ok(());
	}

	// Clean up PID file when we're done, regardless of success or failure.
	let _pid_guard = scopeguard::guard((), |()| remove_pid_file());

	let state = load_state()?;
	let host_hash = compute_client_host_hash();
	let remote_root = &config.sync.remote_root;
	let remote_root_str = remote_root.to_string_lossy().into_owned();

	let client = connect(config, true).await?;

	// Check quota to determine which threshold to apply.
	let quota = check_quota(&client).await?;
	let usage_percent = quota.as_ref().map_or(0.0, QuotaUsage::usage_percent);

	let thresholds = config.clean.effective_thresholds();
	info!(
		usage_percent = format!("{usage_percent:.1}"),
		threshold_count = thresholds.len(),
		"Checking cleanup thresholds"
	);

	// Find the most aggressive threshold that applies for the current quota usage.
	// Thresholds are sorted by percentage ascending (BTreeMap). We want the highest
	// percentage that is <= current usage. If no quota data is available, only
	// the 0% threshold (max_age) applies.
	let applicable_threshold = if quota.is_some() {
		thresholds
			.iter()
			.rev()
			.find(|(pct, _)| {
				let pct_f64 = f64::from(**pct);
				usage_percent >= pct_f64
			})
			.map(|(_, duration)| duration.as_duration())
	} else {
		// No quota available: only apply the 0% threshold (max_age).
		thresholds.get(&0).map(HumanDuration::as_duration)
	};

	let Some(max_age) = applicable_threshold else {
		debug!("No applicable cleanup threshold");
		return Ok(());
	};

	let expired = stale_connections(&state, max_age);
	let stale_from_state: Vec<String> = expired
		.iter()
		.filter(|c| {
			c.host == config.ssh.host
				&& c.user == config.ssh.user
				&& c.port == config.ssh.port
				&& is_default_biwa_remote_dir(&c.remote_dir, remote_root, &host_hash)
		})
		.map(|c| c.remote_dir.clone())
		.collect();

	let tracked: HashSet<String> = state
		.connections
		.iter()
		.filter(|c| {
			c.host == config.ssh.host && c.user == config.ssh.user && c.port == config.ssh.port
		})
		.map(|c| c.remote_dir.clone())
		.collect();

	// Only treat "orphan" remote dirs (present on server but not in local state) as candidates
	// when we have at least one tracked path for this target. If local state is empty or broken,
	// orphan detection is unsafe (could remove active projects).
	let has_tracked_dirs = !tracked.is_empty();

	let listed = list_remote_dirs(&client, &remote_root_str).await?;
	let mut orphan_dirs = Vec::new();
	for name in &listed {
		let full_path = format!("{remote_root_str}/{name}");
		if has_tracked_dirs
			&& is_default_biwa_remote_dir(&full_path, remote_root, &host_hash)
			&& !tracked.contains(&full_path)
		{
			orphan_dirs.push(full_path);
		}
	}

	let mut to_remove: HashSet<String> = HashSet::new();
	for d in stale_from_state {
		to_remove.insert(d);
	}
	for d in orphan_dirs {
		to_remove.insert(d);
	}

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

	if is_daemon_running() {
		debug!("Background cleanup daemon is already running; skipping");
		return Ok(());
	}

	let exe = env::current_exe()?;
	let mut cmd = Command::new(exe);
	cmd.args(["clean", "--auto", "--quiet"]);

	// Forward SSH config via environment so the background process connects
	// to the same host even if it runs from a different working directory.
	for key in &[
		"BIWA_SSH_HOST",
		"BIWA_SSH_PORT",
		"BIWA_SSH_USER",
		"BIWA_SSH_PASSWORD",
	] {
		if let Ok(val) = env::var(key) {
			cmd.env(key, val);
		}
	}

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
