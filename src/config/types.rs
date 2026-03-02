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
	#[config(default = "cse.unsw.edu.au", env = "BIWA_SSH__HOST")]
	pub host: String,
	#[config(default = 22, env = "BIWA_SSH__PORT")]
	pub port: u16,
	#[config(default = "z1234567", env = "BIWA_SSH__USER")]
	pub user: String,
	#[config(env = "BIWA_SSH__KEY_PATH")]
	pub key_path: Option<PathBuf>,
}

#[derive(confique::Config, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncConfig {
	#[config(default = "~/.cache/biwa/projects", env = "BIWA_SYNC__REMOTE_ROOT")]
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
