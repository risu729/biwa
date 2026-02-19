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
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
#[serde(default)]
pub struct SshConfig {
	pub host: String,
	pub port: u16,
	pub user: String,
	pub key_path: Option<String>,
}

impl Default for SshConfig {
	fn default() -> Self {
		Self {
			host: "cse.unsw.edu.au".to_string(),
			port: 22,
			user: "z1234567".to_string(),
			key_path: None,
		}
	}
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
#[serde(default)]
pub struct SyncConfig {
	pub remote_root: String,
	pub ignore_files: Vec<String>,
}

impl Default for SyncConfig {
	fn default() -> Self {
		Self {
			remote_root: "~/.cache/biwa/projects".to_string(),
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
