use crate::Result;
use crate::cache::{
	is_daemon_running, kill_daemon, load_cache, remove_connections, remove_pid_file,
	stale_connections, write_pid_file,
};
use crate::config::types::Config;
use crate::duration::HumanDuration;
use crate::ssh::clean::{QuotaUsage, check_quota, list_remote_dirs, remove_remote_dir};
use crate::ssh::exec::connect;
use crate::ssh::sync::{compute_client_host_hash, compute_project_remote_dir};
use clap::{Args, Subcommand};
use color_eyre::eyre::bail;
use console::style;
use nix::unistd;
use std::io;
use std::process::{Command, Stdio};
use std::{env, fs};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

/// Clean stale remote project directories.
#[derive(Args, Debug)]
#[clap(visible_alias = "c")]
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
	remove_connections(&[remote_dir.as_str()])?;

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
	let cache = load_cache()?;

	// Filter to connections matching current SSH config.
	let matching: Vec<_> = cache
		.connections
		.iter()
		.filter(|c| {
			c.host == config.ssh.host && c.user == config.ssh.user && c.port == config.ssh.port
		})
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

	let mut join_set = JoinSet::new();
	let dirs_to_remove: Vec<String> = matching.iter().map(|c| c.remote_dir.clone()).collect();

	for dir in &dirs_to_remove {
		let client_clone = client.clone();
		let dir_clone = dir.clone();
		join_set.spawn(async move { remove_remote_dir(&client_clone, &dir_clone).await });
	}

	let mut errors = 0_usize;
	while let Some(result) = join_set.join_next().await {
		match result {
			Ok(Ok(())) => {}
			Ok(Err(e)) => {
				warn!(error = %e, "Failed to remove a remote directory");
				errors = errors.saturating_add(1);
			}
			Err(e) => {
				warn!(error = %e, "Task panicked while removing directory");
				errors = errors.saturating_add(1);
			}
		}
	}

	// Remove all successfully cleaned entries from cache.
	let dir_refs: Vec<&str> = dirs_to_remove.iter().map(String::as_str).collect();
	remove_connections(&dir_refs)?;

	if !quiet {
		eprintln!(
			"{} Cleaned {} remote directories ({} errors)",
			style("✓").green().bold(),
			dirs_to_remove.len().saturating_sub(errors),
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

	let mut join_set = JoinSet::new();
	let full_paths: Vec<String> = dirs.iter().map(|d| format!("{remote_root}/{d}")).collect();

	for path in &full_paths {
		let client_clone = client.clone();
		let path_clone = path.clone();
		join_set.spawn(async move { remove_remote_dir(&client_clone, &path_clone).await });
	}

	let mut errors = 0_usize;
	while let Some(result) = join_set.join_next().await {
		match result {
			Ok(Ok(())) => {}
			Ok(Err(e)) => {
				warn!(error = %e, "Failed to remove a remote directory");
				errors = errors.saturating_add(1);
			}
			Err(e) => {
				warn!(error = %e, "Task panicked while removing directory");
				errors = errors.saturating_add(1);
			}
		}
	}

	// Remove matching entries from cache.
	let dir_refs: Vec<&str> = full_paths.iter().map(String::as_str).collect();
	remove_connections(&dir_refs)?;

	if !quiet {
		eprintln!(
			"{} Purged {} directories under {remote_root} ({} errors)",
			style("✓").green().bold(),
			full_paths.len().saturating_sub(errors),
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

	let cache = load_cache()?;
	let host_hash = compute_client_host_hash();

	// Check if any connections match current SSH config and this client's host hash.
	let has_matching = cache.connections.iter().any(|c| {
		c.host == config.ssh.host
			&& c.user == config.ssh.user
			&& c.port == config.ssh.port
			&& c.remote_dir.contains(&host_hash)
	});

	if !has_matching {
		debug!("No stale connections to clean");
		return Ok(());
	}

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

	let stale = stale_connections(&cache, max_age);
	let stale_dirs: Vec<String> = stale
		.iter()
		.filter(|c| {
			c.host == config.ssh.host
				&& c.user == config.ssh.user
				&& c.port == config.ssh.port
				&& c.remote_dir.contains(&host_hash)
		})
		.map(|c| c.remote_dir.clone())
		.collect();

	if stale_dirs.is_empty() {
		debug!("No stale directories to clean");
		return Ok(());
	}

	info!(
		count = stale_dirs.len(),
		max_age_secs = max_age.as_secs(),
		"Cleaning stale remote directories"
	);

	// Remove directories in parallel.
	let mut join_set = JoinSet::new();
	for dir in &stale_dirs {
		let client_clone = client.clone();
		let dir_clone = dir.clone();
		join_set.spawn(async move { remove_remote_dir(&client_clone, &dir_clone).await });
	}

	let mut errors = 0_usize;
	while let Some(result) = join_set.join_next().await {
		match result {
			Ok(Ok(())) => {}
			Ok(Err(e)) => {
				warn!(error = %e, "Failed to remove a stale directory");
				errors = errors.saturating_add(1);
			}
			Err(e) => {
				warn!(error = %e, "Task panicked while removing stale directory");
				errors = errors.saturating_add(1);
			}
		}
	}

	// Remove cleaned entries from cache.
	let dir_refs: Vec<&str> = stale_dirs.iter().map(String::as_str).collect();
	remove_connections(&dir_refs)?;

	info!(
		removed = stale_dirs.len().saturating_sub(errors),
		errors, "Background cleanup completed"
	);

	Ok(())
}

/// Spawns a detached background process to run `biwa clean --auto`.
///
/// The child process is fully detached (new session, stdio to /dev/null) so it
/// survives the parent exiting.
pub fn spawn_background_cleanup() -> Result<()> {
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
		use std::os::unix::process::CommandExt as _;
		// SAFETY: setsid() is safe to call pre-exec; it only affects the child process.
		unsafe {
			cmd.pre_exec(|| {
				unistd::setsid().map_err(io::Error::other)?;
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
