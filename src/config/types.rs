use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
	#[config(nested)]
	pub ssh: SshConfig,
	#[config(nested)]
	pub sync: SyncConfig,
	#[config(nested)]
	pub env: EnvConfig,
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

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshConfig {
	#[config(default = "cse.unsw.edu.au", env = "BIWA_SSH_HOST")]
	pub host: String,
	#[config(default = 22, env = "BIWA_SSH_PORT")]
	pub port: u16,
	#[config(default = "z1234567", env = "BIWA_SSH_USER")]
	pub user: String,
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

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncConfig {
	#[config(default = "~/.cache/biwa/projects", env = "BIWA_SYNC_REMOTE_ROOT")]
	pub remote_root: PathBuf,
	#[config(default = [".git", "target", "node_modules"])]
	pub ignore_files: Vec<PathBuf>,
}

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvConfig {
	#[config(default = [])]
	pub vars: Vec<String>,
}

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HooksConfig {
	pub pre_sync: Option<String>,
	pub post_sync: Option<String>,
}
