use core::fmt;
use core::ops::Deref;
use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

/// Maximum allowed umask value (0o7777 = 4095).
const UMASK_MAX: u32 = 0o7777;

/// Umask value for SSH execution and sync, stored as a normalized octal string.
///
/// Deserializes from either a string (parsed as octal, e.g. `"077"` or `"0022"`) or an integer
/// (the decimal value of the umask, e.g. `63` for 0o77). Always serialized as an octal string.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
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
		Self("0077".to_owned())
	}
}

impl fmt::Display for Umask {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(f)
	}
}

impl Deref for Umask {
	type Target = str;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<'de> Deserialize<'de> for Umask {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		struct UmaskVisitor;

		impl Visitor<'_> for UmaskVisitor {
			type Value = Umask;

			fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
				formatter
					.write_str("umask as octal string (e.g. \"077\") or decimal integer (e.g. 63)")
			}

			fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				let n = u32::from_str_radix(v, 8)
					.map_err(|e| E::custom(format!("Invalid umask (expected octal): {v} ({e})")))?;
				validate_umask(n).map_err(E::custom)?;
				Ok(Umask(format!("{n:04o}")))
			}

			fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				let n = u32::try_from(v)
					.map_err(|e| E::custom(format!("umask value out of range: {e}")))?;
				validate_umask(n).map_err(E::custom)?;
				Ok(Umask(format!("{n:04o}")))
			}

			fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				let n = u32::try_from(v)
					.map_err(|e| E::custom(format!("umask value must be non-negative: {e}")))?;
				validate_umask(n).map_err(E::custom)?;
				Ok(Umask(format!("{n:04o}")))
			}
		}

		deserializer.deserialize_any(UmaskVisitor)
	}
}

/// Ensures the umask value is within the valid range (0..=0o7777).
fn validate_umask(n: u32) -> Result<(), String> {
	if n > UMASK_MAX {
		Err(format!("umask must be between 0 and 0o7777 (got {n:#o})"))
	} else {
		Ok(())
	}
}

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
	/// Umask to apply before executing commands and creating directories.
	/// Note that you cannot loosen the default umask set by the server (e.g., 0027 in UNSW CSE).
	/// You need to use `chmod` manually if you want looser permissions. However, this umask setting
	/// cannot restrict manual permission modifications via `chmod` (be careful with `chmod +x` or `+r`).
	/// Accepts an octal string (e.g. `"077"`, `"0022"`) or a decimal integer (e.g. `63` for 0o77).
	#[config(default = "077", env = "BIWA_SSH_UMASK")]
	pub umask: Umask,
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

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn umask_deserialize_string_octal() {
		let u: Umask = serde_json::from_str(r#""077""#).unwrap();
		assert_eq!(u.as_u32(), 0o77);
		assert_eq!(u.to_string(), "0077");
		let u: Umask = serde_json::from_str(r#""0022""#).unwrap();
		assert_eq!(u.as_u32(), 0o22);
		assert_eq!(u.to_string(), "0022");
	}

	#[test]
	fn umask_deserialize_integer_decimal() {
		// 63 decimal = 0o77
		let u: Umask = serde_json::from_str("63").unwrap();
		assert_eq!(u.as_u32(), 0o77);
		assert_eq!(u.to_string(), "0077");
		// 18 decimal = 0o22
		let u: Umask = serde_json::from_str("18").unwrap();
		assert_eq!(u.as_u32(), 0o22);
		assert_eq!(u.to_string(), "0022");
	}

	#[test]
	fn umask_serialize_string() {
		let u: Umask = serde_json::from_str("18").unwrap();
		let s = serde_json::to_string(&u).unwrap();
		assert_eq!(s, r#""0022""#);
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
		let r: Result<Umask, _> = serde_json::from_str("4096");
		let _: serde_json::Error = r.unwrap_err();
	}
}
