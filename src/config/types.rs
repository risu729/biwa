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
	/// Logging configuration.
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

/// Logging configuration settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogConfig {
	/// Suppresses biwa internal logs; only remote command output is shown.
	#[config(default = false, env = "BIWA_LOG_QUIET")]
	pub quiet: bool,
	/// Suppresses all output, including remote command stdout/stderr.
	#[config(default = false, env = "BIWA_LOG_SILENT")]
	pub silent: bool,
}

/// The synchronization engine to use.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncEngine {
	/// Use SFTP for synchronization.
	#[default]
	Sftp,
	/// Use Mutagen for synchronization.
	Mutagen,
}

/// Strategy for enforcing file permissions during SFTP upload.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SftpPermissions {
	/// Delete the file before creating it with the correct permissions.
	/// This is the most compatible strategy as it works on all SFTP servers.
	#[default]
	Recreate,
	/// Use the SFTP `setstat` operation to set permissions after writing.
	/// Some servers (e.g. UNSW CSE) reject this operation.
	Setstat,
}

/// SFTP synchronization engine settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncSftpConfig {
	/// Abort synchronization if the number of files to upload exceeds this limit.
	#[config(default = 100, env = "BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC")]
	pub max_files_to_sync: usize,
	/// Strategy for enforcing file permissions on uploaded files.
	#[config(default = "recreate", env = "BIWA_SYNC_SFTP_PERMISSIONS")]
	pub permissions: SftpPermissions,
}

/// Synchronization settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncConfig {
	/// Automatically synchronize the project before running remote commands.
	#[config(default = true, env = "BIWA_SYNC_AUTO")]
	pub auto: bool,
	/// Base directory to start the synchronization from. If not specified, uses the current working directory.
	#[config(env = "BIWA_SYNC_ROOT")]
	pub sync_root: Option<PathBuf>,
	/// Remote directory to sync the project to.
	#[config(default = "~/.cache/biwa/projects", env = "BIWA_SYNC_REMOTE_ROOT")]
	pub remote_root: PathBuf,
	/// Files and directories to exclude during synchronization using globset patterns.
	#[config(default = ["**/.git/**", "**/target/**", "**/node_modules/**"])]
	pub exclude: Vec<String>,
	/// The synchronization engine to use.
	#[config(default = "sftp", env = "BIWA_SYNC_ENGINE")]
	pub engine: SyncEngine,
	/// SFTP engine specific configuration.
	#[config(nested)]
	pub sftp: SyncSftpConfig,
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
