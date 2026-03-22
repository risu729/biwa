use crate::Result;
use chrono::{DateTime, Utc};
use color_eyre::eyre::Context as _;
use core::time::Duration;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::{env, process};
use tracing::{debug, warn};

/// A tracked remote project connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedConnection {
	/// Hostname or IP address of the SSH server.
	pub host: String,
	/// Username for the SSH connection.
	pub user: String,
	/// Port used for the SSH connection.
	pub port: u16,
	/// The full remote project directory path.
	pub remote_dir: String,
	/// Timestamp of the last time this connection was used.
	pub last_used: DateTime<Utc>,
}

/// Full cache state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
	/// All tracked remote project connections.
	pub connections: Vec<CachedConnection>,
}

/// Cache filename.
const CACHE_FILE: &str = "connections.json";

/// PID filename for the background cleanup daemon.
pub const PID_FILE: &str = "clean.pid";

/// Returns the biwa cache directory.
///
/// Priority: `$BIWA_CACHE_DIR` > `$XDG_CACHE_HOME/biwa` > `~/.cache/biwa`.
#[must_use]
pub fn cache_dir() -> PathBuf {
	if let Ok(dir) = env::var("BIWA_CACHE_DIR") {
		return PathBuf::from(dir);
	}
	dirs::cache_dir()
		.unwrap_or_else(|| {
			homedir::my_home()
				.ok()
				.flatten()
				.unwrap_or_else(|| PathBuf::from("~"))
				.join(".cache")
		})
		.join("biwa")
}

/// Returns the path to the cache file.
fn cache_file_path() -> PathBuf {
	cache_dir().join(CACHE_FILE)
}

/// Returns the path to the PID file for the cleanup daemon.
#[must_use]
pub fn pid_file_path() -> PathBuf {
	cache_dir().join(PID_FILE)
}

/// Loads the cache from disk, returning an empty cache if the file does not exist.
pub fn load_cache() -> Result<Cache> {
	let path = cache_file_path();
	if !path.exists() {
		return Ok(Cache::default());
	}
	let contents =
		fs::read_to_string(&path).wrap_err_with(|| format!("Failed to read cache: {}", path.display()))?;
	serde_json::from_str(&contents)
		.wrap_err_with(|| format!("Failed to parse cache: {}", path.display()))
}

/// Saves the cache to disk atomically by writing to a temporary file first.
pub fn save_cache(cache: &Cache) -> Result<()> {
	let path = cache_file_path();
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)
			.wrap_err_with(|| format!("Failed to create cache directory: {}", parent.display()))?;
	}
	let contents = serde_json::to_string_pretty(cache)
		.wrap_err("Failed to serialize cache")?;
	let tmp_path = path.with_extension("json.tmp");
	fs::write(&tmp_path, &contents)
		.wrap_err_with(|| format!("Failed to write cache: {}", tmp_path.display()))?;
	fs::rename(&tmp_path, &path)
		.wrap_err_with(|| format!("Failed to rename cache: {} -> {}", tmp_path.display(), path.display()))?;
	debug!(path = %path.display(), "Saved cache");
	Ok(())
}

/// Records a connection in the cache, upserting by `(host, user, port, remote_dir)`.
pub fn record_connection(
	host: &str,
	user: &str,
	port: u16,
	remote_dir: &str,
) -> Result<()> {
	let mut cache = load_cache()?;
	let now = Utc::now();

	if let Some(existing) = cache.connections.iter_mut().find(|c| {
		c.host == host && c.user == user && c.port == port && c.remote_dir == remote_dir
	}) {
		existing.last_used = now;
		debug!(
			host,
			user,
			port,
			remote_dir,
			"Updated existing connection in cache"
		);
	} else {
		cache.connections.push(CachedConnection {
			host: host.to_owned(),
			user: user.to_owned(),
			port,
			remote_dir: remote_dir.to_owned(),
			last_used: now,
		});
		debug!(
			host,
			user,
			port,
			remote_dir,
			"Added new connection to cache"
		);
	}

	save_cache(&cache)
}

/// Returns connections that have not been used within the given threshold.
#[must_use]
pub fn stale_connections(cache: &Cache, threshold: Duration) -> Vec<&CachedConnection> {
	let now = Utc::now();
	cache
		.connections
		.iter()
		.filter(|c| {
			let age = now.signed_duration_since(c.last_used);
			// Convert chrono::Duration to std::time::Duration for comparison.
			age.to_std().is_ok_and(|std_age| std_age > threshold)
		})
		.collect()
}

/// Removes connections matching the given remote directories from the cache.
pub fn remove_connections(remote_dirs: &[&str]) -> Result<()> {
	let mut cache = load_cache()?;
	let before = cache.connections.len();
	cache
		.connections
		.retain(|c| !remote_dirs.contains(&c.remote_dir.as_str()));
	let removed = before.saturating_sub(cache.connections.len());
	if removed > 0 {
		debug!(removed, "Removed connections from cache");
		save_cache(&cache)?;
	}
	Ok(())
}

/// Writes the current process PID to the PID file.
///
/// Returns `true` if a daemon was already running (PID file existed with a live process).
pub fn write_pid_file() -> Result<bool> {
	let path = pid_file_path();
	let already_running = is_daemon_running();

	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)
			.wrap_err_with(|| format!("Failed to create PID directory: {}", parent.display()))?;
	}

	let pid = process::id();
	fs::write(&path, pid.to_string())
		.wrap_err_with(|| format!("Failed to write PID file: {}", path.display()))?;
	debug!(pid, path = %path.display(), "Wrote PID file");

	Ok(already_running)
}

/// Removes the PID file.
pub fn remove_pid_file() {
	let path = pid_file_path();
	if let Err(e) = fs::remove_file(&path)
		&& e.kind() != io::ErrorKind::NotFound
	{
		warn!(error = %e, path = %path.display(), "Failed to remove PID file");
	}
}

/// Returns `true` if a cleanup daemon is currently running.
#[must_use]
pub fn is_daemon_running() -> bool {
	read_daemon_pid().is_some_and(|pid| {
		signal::kill(Pid::from_raw(pid), None).is_ok()
	})
}

/// Reads the daemon PID from the PID file.
#[must_use]
pub fn read_daemon_pid() -> Option<i32> {
	let path = pid_file_path();
	let Ok(contents) = fs::read_to_string(&path) else {
		return None;
	};
	contents.trim().parse::<i32>().ok()
}

/// Kills the running cleanup daemon if one exists.
pub fn kill_daemon() {
	if let Some(pid) = read_daemon_pid() {
		let nix_pid = Pid::from_raw(pid);
		if signal::kill(nix_pid, None).is_ok() {
			debug!(pid, "Sending SIGTERM to cleanup daemon");
			if let Err(e) = signal::kill(nix_pid, Signal::SIGTERM) {
				warn!(error = %e, pid, "Failed to kill cleanup daemon");
			}
		}
	}
	remove_pid_file();
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::EnvCleanup;
	use pretty_assertions::assert_eq;
	use serial_test::serial;
	use std::thread;

	#[test]
	#[serial]
	fn record_and_load_connection() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_CACHE_DIR", dir.path().to_str().unwrap());

		record_connection("host", "user", 22, "/remote/dir").unwrap();
		let cache = load_cache().unwrap();
		assert_eq!(cache.connections.len(), 1);
		let first = cache.connections.first().unwrap();
		assert_eq!(first.host, "host");
		assert_eq!(first.remote_dir, "/remote/dir");
	}

	#[test]
	#[serial]
	fn upsert_updates_timestamp() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_CACHE_DIR", dir.path().to_str().unwrap());

		record_connection("host", "user", 22, "/remote/dir").unwrap();
		let cache1 = load_cache().unwrap();
		let ts1 = cache1.connections.first().unwrap().last_used;

		// Tiny delay so timestamp differs.
		thread::sleep(Duration::from_millis(10));
		record_connection("host", "user", 22, "/remote/dir").unwrap();
		let cache2 = load_cache().unwrap();
		assert_eq!(cache2.connections.len(), 1);
		assert!(cache2.connections.first().unwrap().last_used >= ts1);
	}

	#[test]
	#[serial]
	fn stale_connections_filters_by_age() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_CACHE_DIR", dir.path().to_str().unwrap());

		let mut cache = Cache::default();
		cache.connections.push(CachedConnection {
			host: "host".to_owned(),
			user: "user".to_owned(),
			port: 22,
			remote_dir: "/old".to_owned(),
			last_used: Utc::now() - chrono::Duration::days(31),
		});
		cache.connections.push(CachedConnection {
			host: "host".to_owned(),
			user: "user".to_owned(),
			port: 22,
			remote_dir: "/new".to_owned(),
			last_used: Utc::now(),
		});
		let stale = stale_connections(&cache, Duration::from_secs(30 * 86400));
		assert_eq!(stale.len(), 1);
		assert_eq!(stale.first().unwrap().remote_dir, "/old");
	}

	#[test]
	#[serial]
	fn remove_connections_from_cache() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_CACHE_DIR", dir.path().to_str().unwrap());

		record_connection("host", "user", 22, "/dir1").unwrap();
		record_connection("host", "user", 22, "/dir2").unwrap();
		remove_connections(&["/dir1"]).unwrap();
		let cache = load_cache().unwrap();
		assert_eq!(cache.connections.len(), 1);
		assert_eq!(cache.connections.first().unwrap().remote_dir, "/dir2");
	}
}
