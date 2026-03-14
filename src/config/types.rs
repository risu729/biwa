use crate::env_vars::{EnvForwardMethod, EnvVars};
use core::fmt;
use core::str::FromStr;
use derive_more::Deref;
use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

/// Maximum allowed umask value (0o777 = 511). Three digits (owner/group/other) only.
const UMASK_MAX: u32 = 0o777;

/// Umask value for SSH execution and sync, stored as a normalized 3-digit octal string.
///
/// Deserializes from a string parsed as octal (e.g. `"077"`, `"022"`). Only the lower three
/// digits (owner/group/other) are supported. To set the first digit (setuid/setgid/sticky),
/// run `umask` manually on the remote server. Always serialized as a 3-digit octal string.
#[derive(Debug, Clone, Deref, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct Umask(String);

impl Umask {
	/// Returns the umask as a `u32` (e.g. for permission calculations).
	#[must_use]
	pub fn as_u32(&self) -> u32 {
		// Validated at deserialization; only valid octal digits.
		u32::from_str_radix(&self.0, 8).expect("umask stored as valid octal")
	}
}

impl Default for Umask {
	fn default() -> Self {
		Self("077".to_owned())
	}
}

impl fmt::Display for Umask {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(f)
	}
}

impl FromStr for Umask {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let n = u32::from_str_radix(s, 8)
			.map_err(|e| format!("Invalid umask (expected octal): {s} ({e})"))?;
		validate_umask(n)?;
		Ok(Self(format!("{n:03o}")))
	}
}

impl<'de> Deserialize<'de> for Umask {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let s = String::deserialize(deserializer)?;
		Self::from_str(&s).map_err(D::Error::custom)
	}
}

/// Ensures the umask value is within the valid range (0..=0o777).
fn validate_umask(n: u32) -> Result<(), String> {
	if n > UMASK_MAX {
		Err(format!("umask must be between 0 and 0o777 (got {n:#o})"))
	} else {
		Ok(())
	}
}

/// Default values for schema generation; must match `#[config(default = ...)]` in this module.
mod schema_defaults {
	use std::path::PathBuf;

	/// Default for `SshConfig::host` in schema.
	#[must_use]
	pub fn ssh_host() -> String {
		"cse.unsw.edu.au".to_owned()
	}

	/// Default for `SshConfig::port` in schema.
	#[must_use]
	pub const fn ssh_port() -> u16 {
		22
	}

	/// Default for `SshConfig::user` in schema.
	#[must_use]
	pub fn ssh_user() -> String {
		"z1234567".to_owned()
	}

	/// Default for `SyncConfig::remote_root` in schema.
	#[must_use]
	pub fn sync_remote_root() -> PathBuf {
		PathBuf::from("~/.cache/biwa/projects")
	}

	/// Default for `SyncConfig::exclude` in schema.
	#[must_use]
	pub fn sync_exclude() -> Vec<String> {
		vec![
			"**/.git/**".to_owned(),
			"**/target/**".to_owned(),
			"**/node_modules/**".to_owned(),
		]
	}

	/// Default for `SyncConfig::auto` in schema.
	#[must_use]
	pub const fn sync_auto() -> bool {
		true
	}

	/// Default for `SyncSftpConfig::max_files_to_sync` in schema.
	#[must_use]
	pub const fn max_files_to_sync() -> usize {
		100
	}
}

/// Root configuration struct for biwa.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
	/// SSH connection configuration.
	#[config(nested)]
	#[schemars(default)]
	pub ssh: SshConfig,
	/// Remote synchronization configuration.
	#[config(nested)]
	#[schemars(default)]
	pub sync: SyncConfig,
	/// Environment variables configuration.
	#[config(nested)]
	#[schemars(default)]
	pub env: EnvConfig,
	/// Lifecycle hooks for synchronization.
	#[config(nested)]
	#[schemars(default)]
	pub hooks: HooksConfig,
	/// Logging configuration.
	#[config(nested)]
	#[schemars(default)]
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
	#[schemars(default = "crate::config::types::schema_defaults::ssh_host")]
	pub host: String,
	/// Port to connect to on the remote host.
	#[config(default = 22, env = "BIWA_SSH_PORT")]
	#[schemars(default = "crate::config::types::schema_defaults::ssh_port")]
	pub port: u16,
	/// Username for the SSH connection.
	#[config(default = "z1234567", env = "BIWA_SSH_USER")]
	#[schemars(default = "crate::config::types::schema_defaults::ssh_user")]
	pub user: String,
	/// Optional path to the SSH private key.
	#[config(env = "BIWA_SSH_KEY_PATH")]
	pub key_path: Option<PathBuf>,
	/// Password authentication: `false` (default), `true` (prompt), or a string value.
	#[config(default = false, env = "BIWA_SSH_PASSWORD")]
	#[schemars(default)]
	pub password: PasswordConfig,
	/// Umask to apply before executing commands and creating directories (3-digit octal: owner/group/other).
	/// To set the first digit (setuid/setgid/sticky), run `umask` manually on the remote server.
	/// Note that you cannot loosen the default umask set by the server (e.g., 027 in UNSW CSE).
	/// You need to use `chmod` manually if you want looser permissions. However, this umask setting
	/// cannot restrict manual permission modifications via `chmod` (be careful with `chmod +x` or `+r`).
	#[config(default = "077", env = "BIWA_SSH_UMASK")]
	#[schemars(default)]
	pub umask: Umask,
}

impl Default for SshConfig {
	fn default() -> Self {
		Config::default().ssh
	}
}

/// Logging configuration settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogConfig {
	/// Suppresses biwa internal logs; only remote command output is shown.
	#[config(default = false, env = "BIWA_LOG_QUIET")]
	#[schemars(default)]
	pub quiet: bool,
	/// Suppresses all output, including remote command stdout/stderr.
	#[config(default = false, env = "BIWA_LOG_SILENT")]
	#[schemars(default)]
	pub silent: bool,
}

impl Default for LogConfig {
	fn default() -> Self {
		Config::default().log
	}
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
	#[schemars(default = "crate::config::types::schema_defaults::max_files_to_sync")]
	pub max_files_to_sync: usize,
	/// Strategy for enforcing file permissions on uploaded files.
	#[config(default = "recreate", env = "BIWA_SYNC_SFTP_PERMISSIONS")]
	#[schemars(default)]
	pub permissions: SftpPermissions,
}

impl Default for SyncSftpConfig {
	fn default() -> Self {
		Config::default().sync.sftp
	}
}

/// Synchronization settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncConfig {
	/// Automatically synchronize the project before running remote commands.
	#[config(default = true, env = "BIWA_SYNC_AUTO")]
	#[schemars(default = "crate::config::types::schema_defaults::sync_auto")]
	pub auto: bool,
	/// Base directory to start the synchronization from. If not specified, uses the current working directory.
	#[config(env = "BIWA_SYNC_ROOT")]
	pub sync_root: Option<PathBuf>,
	/// Remote directory to sync the project to.
	#[config(default = "~/.cache/biwa/projects", env = "BIWA_SYNC_REMOTE_ROOT")]
	#[schemars(default = "crate::config::types::schema_defaults::sync_remote_root")]
	pub remote_root: PathBuf,
	/// Files and directories to exclude during synchronization using globset patterns.
	#[config(default = ["**/.git/**", "**/target/**", "**/node_modules/**"])]
	#[schemars(default = "crate::config::types::schema_defaults::sync_exclude")]
	pub exclude: Vec<String>,
	/// The synchronization engine to use.
	#[config(default = "sftp", env = "BIWA_SYNC_ENGINE")]
	#[schemars(default)]
	pub engine: SyncEngine,
	/// SFTP engine specific configuration.
	#[config(nested)]
	#[schemars(default)]
	pub sftp: SyncSftpConfig,
}

impl Default for SyncConfig {
	fn default() -> Self {
		Config::default().sync
	}
}

/// Environment settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvConfig {
	/// Environment variables to send to the remote process.
	///
	/// Supports exact names and values such as
	/// `vars = ["NODE_ENV", "API_KEY=secret"]`, wildcard rules such as
	/// `vars = ["NODE_*", "!*PATH"]`, and `[env.vars]` table forms.
	#[config(default = [])]
	#[schemars(default)]
	pub vars: EnvVars,
	/// Forwarding strategy for environment variables.
	#[config(default = "export", env = "BIWA_ENV_FORWARD_METHOD")]
	#[schemars(default)]
	pub forward_method: EnvForwardMethod,
}

impl Default for EnvConfig {
	fn default() -> Self {
		Config::default().env
	}
}

/// Hook settings.
#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HooksConfig {
	/// Command to run before synchronization.
	pub pre_sync: Option<String>,
	/// Command to run after synchronization.
	pub post_sync: Option<String>,
}

impl Default for HooksConfig {
	fn default() -> Self {
		Config::default().hooks
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec::Vec;
	use pretty_assertions::assert_eq;
	use schemars::schema_for;

	/// Ensures the generated JSON schema marks fields with defaults as optional (no required).
	#[test]
	fn schema_defaulted_fields_optional() {
		let schema = schema_for!(Config);
		let value = serde_json::to_value(&schema).expect("schema serializes to JSON");
		// Root Config: no required keys (partial configs allowed).
		let root_required = value.get("required").and_then(|v| v.as_array());
		assert!(
			root_required.is_none_or(Vec::is_empty),
			"root schema must not require any keys; got required = {root_required:?}"
		);
		// SshConfig: fields with defaults (e.g. host) must not be in required.
		let defs = value
			.get("$defs")
			.and_then(|v| v.as_object())
			.expect("schema has $defs");
		let ssh = defs
			.get("SshConfig")
			.and_then(|v| v.as_object())
			.expect("SshConfig in $defs");
		let required = ssh.get("required").and_then(|v| v.as_array());
		if let Some(r) = required {
			assert!(
				!r.iter().any(|v| v.as_str() == Some("host")),
				"SshConfig.required must not contain 'host'; required = {r:?}"
			);
		}
	}

	#[test]
	fn umask_deserialize_string_octal() {
		let u: Umask = serde_json::from_str(r#""077""#).unwrap();
		assert_eq!(u.as_u32(), 0o77);
		assert_eq!(u.to_string(), "077");
		let u: Umask = serde_json::from_str(r#""022""#).unwrap();
		assert_eq!(u.as_u32(), 0o22);
		assert_eq!(u.to_string(), "022");
	}

	#[test]
	fn umask_serialize_string() {
		let u: Umask = serde_json::from_str(r#""022""#).unwrap();
		let s = serde_json::to_string(&u).unwrap();
		assert_eq!(s, r#""022""#);
	}

	#[test]
	fn umask_invalid_string_rejected() {
		let r: Result<Umask, _> = serde_json::from_str(r#""09""#);
		let _: serde_json::Error = r.unwrap_err();
		let r: Result<Umask, _> = serde_json::from_str(r#""not-a-number""#);
		let _: serde_json::Error = r.unwrap_err();
	}

	#[test]
	fn umask_out_of_range_rejected() {
		let r: Result<Umask, _> = serde_json::from_str(r#""1000""#);
		let _: serde_json::Error = r.unwrap_err();
	}
}
