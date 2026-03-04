use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Root configuration struct for biwa.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
	/// SSH connection configuration.
	#[config(nested)]
	pub ssh: SshConfig,
	/// Remote synchronization configuration.
	#[config(nested)]
	pub sync: SyncConfig,
	/// Environment variables configuration.
	#[config(nested)]
	pub env: EnvConfig,
	/// Lifecycle hooks for synchronization.
	#[config(nested)]
	pub hooks: HooksConfig,
	#[config(nested)]
	pub log: LogConfig,
}

/// Password authentication configuration.
///
/// - `false` (default): No password authentication.
/// - `true`: Interactively prompt for a password.
/// - `"string"`: Use the provided password value.
#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
#[serde(untagged)]
pub enum PasswordConfig {
	/// Interactive prompt (`true`) or disabled (`false`).
	Interactive(bool),
	/// A literal password value.
	Value(String),
}

impl Default for PasswordConfig {
	fn default() -> Self {
		Self::Interactive(false)
	}
}

impl Default for Config {
	fn default() -> Self {
		confique::Config::builder()
			.load()
			.expect("Failed to build default config")
	}
}

/// SSH connection settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshConfig {
	/// Hostname or IP address of the remote host.
	#[config(default = "cse.unsw.edu.au", env = "BIWA_SSH_HOST")]
	pub host: String,
	/// Port to connect to on the remote host.
	#[config(default = 22, env = "BIWA_SSH_PORT")]
	pub port: u16,
	/// Username for the SSH connection.
	#[config(default = "z1234567", env = "BIWA_SSH_USER")]
	pub user: String,
	/// Optional path to the SSH private key.
	#[config(env = "BIWA_SSH_KEY_PATH")]
	pub key_path: Option<PathBuf>,
	/// Password authentication: `false` (default), `true` (prompt), or a string value.
	#[config(default = false, env = "BIWA_SSH_PASSWORD")]
	pub password: PasswordConfig,
}

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogConfig {
	/// Suppresses biwa internal logs; only remote command output is shown.
	#[config(default = false, env = "BIWA_LOG_QUIET")]
	pub quiet: bool,
	/// Suppresses all output, including remote command stdout/stderr.
	#[config(default = false, env = "BIWA_LOG_SILENT")]
	pub silent: bool,
}

/// Synchronization settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncConfig {
	/// Remote directory to sync the project to.
	#[config(default = "~/.cache/biwa/projects", env = "BIWA_SYNC_REMOTE_ROOT")]
	pub remote_root: PathBuf,
	/// Files and directories to ignore during synchronization.
	#[config(default = [".git", "target", "node_modules"])]
	pub ignore_files: Vec<PathBuf>,
}

/// Environment settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvConfig {
	/// Environment variable keys to inherit/send.
	#[config(default = [])]
	pub vars: Vec<String>,
}

/// Hook settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HooksConfig {
	/// Command to run before synchronization.
	pub pre_sync: Option<String>,
	/// Command to run after synchronization.
	pub post_sync: Option<String>,
}
