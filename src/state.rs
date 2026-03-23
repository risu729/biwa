use crate::Result;
use chrono::{DateTime, Utc};
use color_eyre::eyre::Context as _;
use core::time::Duration;
use nix::errno::Errno;
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
pub struct Connection {
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

/// Persisted connection tracking state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
	/// All tracked remote project connections.
	pub connections: Vec<Connection>,
}

/// Connections list filename under the state directory.
const CONNECTIONS_FILE: &str = "connections.json";

/// PID filename for the background cleanup daemon.
pub const PID_FILE: &str = "clean.pid";

/// Returns the biwa state directory.
///
/// Priority: `$BIWA_STATE_DIR` > `dirs::state_dir()` with `biwa` appended (see the `dirs` crate
/// for platform-specific resolution, e.g. XDG state paths on Linux).
#[must_use]
pub fn state_dir() -> PathBuf {
	if let Ok(dir) = env::var("BIWA_STATE_DIR") {
		return PathBuf::from(dir);
	}
	dirs::state_dir()
		.or_else(|| {
			homedir::my_home()
				.ok()
				.flatten()
				.map(|home| home.join(".local/state"))
		})
		.unwrap_or_else(|| {
			warn!("Could not resolve state directory; using cwd/.local/state/biwa");
			env::current_dir()
				.unwrap_or_else(|_| PathBuf::from("."))
				.join(".local/state")
		})
		.join("biwa")
}

/// Returns the path to the connections file.
fn connections_file_path() -> PathBuf {
	state_dir().join(CONNECTIONS_FILE)
}

/// Returns the path to the PID file for the cleanup daemon.
#[must_use]
pub fn pid_file_path() -> PathBuf {
	state_dir().join(PID_FILE)
}

/// Loads persisted state from disk, returning empty state if the file does not exist.
pub fn load_state() -> Result<State> {
	let path = connections_file_path();
	if !path.exists() {
		return Ok(State::default());
	}
	let contents = fs::read_to_string(&path)
		.wrap_err_with(|| format!("Failed to read state: {}", path.display()))?;
	serde_json::from_str(&contents)
		.wrap_err_with(|| format!("Failed to parse state: {}", path.display()))
}

/// Saves state to disk atomically by writing to a temporary file first.
pub fn save_state(state: &State) -> Result<()> {
	let path = connections_file_path();
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)
			.wrap_err_with(|| format!("Failed to create state directory: {}", parent.display()))?;
	}
	let contents = serde_json::to_string_pretty(state).wrap_err("Failed to serialize state")?;
	let tmp_path = path.with_extension("json.tmp");
	fs::write(&tmp_path, &contents)
		.wrap_err_with(|| format!("Failed to write state: {}", tmp_path.display()))?;
	fs::rename(&tmp_path, &path).wrap_err_with(|| {
		format!(
			"Failed to rename state file: {} -> {}",
			tmp_path.display(),
			path.display()
		)
	})?;
	debug!(path = %path.display(), "Saved state");
	Ok(())
}

/// Records a connection, upserting by `(host, user, port, remote_dir)`.
pub fn record_connection(host: &str, user: &str, port: u16, remote_dir: &str) -> Result<()> {
	let mut state = load_state()?;
	let now = Utc::now();

	if let Some(existing) = state
		.connections
		.iter_mut()
		.find(|c| c.host == host && c.user == user && c.port == port && c.remote_dir == remote_dir)
	{
		existing.last_used = now;
		debug!(
			host,
			user, port, remote_dir, "Updated existing connection in state"
		);
	} else {
		state.connections.push(Connection {
			host: host.to_owned(),
			user: user.to_owned(),
			port,
			remote_dir: remote_dir.to_owned(),
			last_used: now,
		});
		debug!(
			host,
			user, port, remote_dir, "Added new connection to state"
		);
	}

	save_state(&state)
}

/// Returns connections that have not been used within the given threshold.
#[must_use]
pub fn stale_connections(state: &State, threshold: Duration) -> Vec<&Connection> {
	let now = Utc::now();
	state
		.connections
		.iter()
		.filter(|c| {
			let age = now.signed_duration_since(c.last_used);
			// Convert chrono::Duration to std::time::Duration for comparison.
			age.to_std().is_ok_and(|std_age| std_age > threshold)
		})
		.collect()
}

/// Removes connections for `remote_dirs` that match the given SSH target.
///
/// `remote_dir` alone is not unique across hosts or accounts; entries for other
/// targets are preserved.
pub fn remove_connections_for_target(
	host: &str,
	user: &str,
	port: u16,
	remote_dirs: &[&str],
) -> Result<()> {
	let mut state = load_state()?;
	let before = state.connections.len();
	state.connections.retain(|c| {
		if c.host == host && c.user == user && c.port == port {
			!remote_dirs.contains(&c.remote_dir.as_str())
		} else {
			true
		}
	});
	let removed = before.saturating_sub(state.connections.len());
	if removed > 0 {
		debug!(removed, "Removed connections from state");
		save_state(&state)?;
	}
	Ok(())
}

/// Writes the current process PID to the PID file.
///
/// Returns `true` if a daemon was already running (does not write the PID file in that case).
pub fn write_pid_file() -> Result<bool> {
	if is_daemon_running() {
		return Ok(true);
	}

	let path = pid_file_path();

	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)
			.wrap_err_with(|| format!("Failed to create PID directory: {}", parent.display()))?;
	}

	let pid = process::id();
	fs::write(&path, pid.to_string())
		.wrap_err_with(|| format!("Failed to write PID file: {}", path.display()))?;
	debug!(pid, path = %path.display(), "Wrote PID file");

	Ok(false)
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
///
/// Uses `kill(pid, 0)` to probe the process. `ESRCH` means the process does
/// not exist; any other error (including `EPERM`) is treated as "still running"
/// to avoid accidentally spawning a second daemon.
#[must_use]
pub fn is_daemon_running() -> bool {
	read_daemon_pid().is_some_and(|pid| {
		match signal::kill(Pid::from_raw(pid), None) {
			// ESRCH: no such process — definitely not running.
			Err(Errno::ESRCH) => false,
			// Ok or any other error (e.g. EPERM): process exists.
			Ok(()) | Err(_) => true,
		}
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
	let Some(pid) = read_daemon_pid() else {
		return;
	};
	let nix_pid = Pid::from_raw(pid);
	match signal::kill(nix_pid, None) {
		Ok(()) => {
			debug!(pid, "Sending SIGTERM to cleanup daemon");
			match signal::kill(nix_pid, Signal::SIGTERM) {
				Ok(()) => remove_pid_file(),
				Err(e) => {
					warn!(
						error = %e,
						pid,
						"Failed to send SIGTERM to cleanup daemon; keeping PID file"
					);
				}
			}
		}
		Err(Errno::ESRCH) => {
			debug!(pid, "Stale cleanup daemon PID; removing PID file");
			remove_pid_file();
		}
		Err(e) => {
			warn!(
				error = %e,
				pid,
				"Cannot probe cleanup daemon process; keeping PID file"
			);
		}
	}
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
		let _cleanup = EnvCleanup::set("BIWA_STATE_DIR", dir.path().to_str().unwrap());

		record_connection("host", "user", 22, "/remote/dir").unwrap();
		let state = load_state().unwrap();
		assert_eq!(state.connections.len(), 1);
		let first = state.connections.first().unwrap();
		assert_eq!(first.host, "host");
		assert_eq!(first.remote_dir, "/remote/dir");
	}

	#[test]
	#[serial]
	fn upsert_updates_timestamp() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_STATE_DIR", dir.path().to_str().unwrap());

		record_connection("host", "user", 22, "/remote/dir").unwrap();
		let state1 = load_state().unwrap();
		let ts1 = state1.connections.first().unwrap().last_used;

		// Tiny delay so timestamp differs.
		thread::sleep(Duration::from_millis(10));
		record_connection("host", "user", 22, "/remote/dir").unwrap();
		let state2 = load_state().unwrap();
		assert_eq!(state2.connections.len(), 1);
		assert!(state2.connections.first().unwrap().last_used >= ts1);
	}

	#[test]
	#[serial]
	fn stale_connections_filters_by_age() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_STATE_DIR", dir.path().to_str().unwrap());

		let mut state = State::default();
		state.connections.push(Connection {
			host: "host".to_owned(),
			user: "user".to_owned(),
			port: 22,
			remote_dir: "/old".to_owned(),
			last_used: Utc::now() - chrono::Duration::days(31),
		});
		state.connections.push(Connection {
			host: "host".to_owned(),
			user: "user".to_owned(),
			port: 22,
			remote_dir: "/new".to_owned(),
			last_used: Utc::now(),
		});
		let stale = stale_connections(&state, Duration::from_secs(30 * 86400));
		assert_eq!(stale.len(), 1);
		assert_eq!(stale.first().unwrap().remote_dir, "/old");
	}

	#[test]
	#[serial]
	fn remove_connections_from_state() {
		let dir = tempfile::tempdir().unwrap();
		let _cleanup = EnvCleanup::set("BIWA_STATE_DIR", dir.path().to_str().unwrap());

		record_connection("host", "user", 22, "/dir1").unwrap();
		record_connection("host", "user", 22, "/dir2").unwrap();
		remove_connections_for_target("host", "user", 22, &["/dir1"]).unwrap();
		let state = load_state().unwrap();
		assert_eq!(state.connections.len(), 1);
		assert_eq!(state.connections.first().unwrap().remote_dir, "/dir2");
	}
}
