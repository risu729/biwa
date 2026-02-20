use figment::value::magic::RelativePathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
	pub ssh: SshConfig,
	pub sync: SyncConfig,
	pub env: EnvConfig,
	pub hooks: HooksConfig,
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

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
#[serde(default)]
pub struct SshConfig {
	pub host: String,
	pub port: u16,
	pub user: String,
	#[schemars(with = "Option<String>")]
	pub key_path: Option<RelativePathBuf>,
	/// Password authentication: `false` (default), `true` (prompt), or a string value.
	pub password: PasswordConfig,
}

impl Default for SshConfig {
	fn default() -> Self {
		Self {
			host: "cse.unsw.edu.au".to_string(),
			port: 22,
			user: "z5555555".to_string(),
			key_path: None,
			password: PasswordConfig::default(),
		}
	}
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone, Default)]
#[serde(default)]
pub struct LogConfig {
	/// Suppresses biwa internal logs; only remote command output is shown.
	pub quiet: bool,
	/// Suppresses all output, including remote command stdout/stderr.
	pub silent: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
#[serde(default)]
pub struct SyncConfig {
	#[serde(serialize_with = "RelativePathBuf::serialize_relative")]
	#[schemars(with = "String")]
	pub remote_root: RelativePathBuf,
	pub ignore_files: Vec<String>,
}

impl Default for SyncConfig {
	fn default() -> Self {
		Self {
			remote_root: RelativePathBuf::from(".cache/biwa/projects"),
			ignore_files: vec![
				".git".to_string(),
				"target".to_string(),
				"node_modules".to_string(),
			],
		}
	}
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone, Default)]
#[serde(default)]
pub struct EnvConfig {
	pub vars: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone, Default)]
#[serde(default)]
pub struct HooksConfig {
	pub pre_sync: Option<String>,
	pub post_sync: Option<String>,
}
