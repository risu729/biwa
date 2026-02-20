use crate::config::Config;
use crate::config::types::PasswordConfig;
use async_ssh2_tokio::client::AuthMethod;
use dialoguer::Password;
use std::path::PathBuf;
use tracing::{debug, info};

/// Default SSH key paths to try when no explicit `key_path` is configured.
const DEFAULT_KEY_PATHS: &[&str] = &["~/.ssh/id_ed25519", "~/.ssh/id_rsa"];

/// Resolve the authentication method based on configuration.
///
/// Auth cascade (explicit configuration is always respected first):
/// 1. Explicit key file (`ssh.key_path`) — errors if file not found
/// 2. Explicit password (`ssh.password = "..."` or `ssh.password = true` for prompt)
/// 3. Default key file discovery (`~/.ssh/id_ed25519`, `~/.ssh/id_rsa`)
/// 4. SSH Agent (fallback for zero-config users)
pub fn resolve_auth(config: &Config) -> eyre::Result<AuthMethod> {
	let ssh = &config.ssh;

	// 1. Explicit key_path (paths are already resolved natively by Figment's RelativePathBuf)
	if let Some(key_path) = &ssh.key_path {
		let path = key_path.relative();
		if path.exists() {
			info!(path = %path.display(), "Using configured SSH key file");
			return Ok(AuthMethod::with_key_file(
				path.to_string_lossy().as_ref(),
				None,
			));
		}
		eyre::bail!("Configured SSH key file not found: {}", path.display());
	}

	// 2. Explicit password (string value or interactive prompt)
	match &ssh.password {
		PasswordConfig::Value(password) => {
			info!("Using password authentication from config");
			return Ok(AuthMethod::with_password(password));
		}
		PasswordConfig::Interactive(true) => {
			info!("Prompting for password (ssh.password = true)");
			let password = Password::new()
				.with_prompt(format!("Password for {}@{}", ssh.user, ssh.host))
				.interact()?;
			return Ok(AuthMethod::with_password(&password));
		}
		PasswordConfig::Interactive(false) => {
			debug!("Password authentication disabled");
		}
	}

	// 3. Try default key file paths
	if let Some(key_path) = resolve_default_key_path() {
		info!(path = %key_path.display(), "Using default SSH key file");
		return Ok(AuthMethod::with_key_file(
			key_path.to_string_lossy().as_ref(),
			None,
		));
	}

	// 4. SSH Agent as last resort (for zero-config users)
	if try_agent() {
		info!("Using SSH agent authentication");
		return Ok(AuthMethod::with_agent());
	}

	eyre::bail!(
		"No authentication method available. \
		Configure ssh.key_path, ssh.password, or set up an SSH agent."
	)
}

/// Check if an SSH agent is available.
fn try_agent() -> bool {
	if !cfg!(unix) {
		debug!("SSH agent is only supported on Unix");
		return false;
	}

	match std::env::var("SSH_AUTH_SOCK") {
		Ok(sock) if !sock.is_empty() => {
			debug!(sock = %sock, "SSH agent socket found");
			true
		}
		_ => {
			debug!("SSH_AUTH_SOCK not set, skipping agent auth");
			false
		}
	}
}

/// Try to find a default SSH key file.
fn resolve_default_key_path() -> Option<PathBuf> {
	let home = homedir::my_home().ok().flatten()?;

	for default_path in DEFAULT_KEY_PATHS {
		let expanded = if let Some(stripped) = default_path.strip_prefix("~/") {
			home.join(stripped)
		} else {
			PathBuf::from(default_path)
		};

		if expanded.exists() {
			debug!(path = %expanded.display(), "Found default SSH key");
			return Some(expanded);
		}
	}

	debug!("No default SSH key file found");
	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::SshConfig;
	use figment::value::magic::RelativePathBuf;

	fn config_with_ssh(ssh: SshConfig) -> Config {
		Config {
			ssh,
			..Default::default()
		}
	}

	#[test]
	fn test_resolve_default_key_path_explicit() {
		let dir = tempfile::tempdir().unwrap();
		let key_file = dir.path().join("my_key");
		std::fs::write(&key_file, "fake key").unwrap();

		let config = config_with_ssh(SshConfig {
			key_path: Some(RelativePathBuf::from(&key_file)),
			..Default::default()
		});

		let result = resolve_auth(&config);
		assert!(result.is_ok());
	}

	#[test]
	fn test_resolve_auth_missing_explicit_key_errors() {
		let config = config_with_ssh(SshConfig {
			key_path: Some(RelativePathBuf::from("/nonexistent/path/key".to_string())),
			..Default::default()
		});

		let result = resolve_auth(&config);
		assert!(result.is_err());
		let err_msg = result.unwrap_err().to_string();
		assert!(err_msg.contains("not found"), "Error: {err_msg}");
	}

	#[test]
	fn test_resolve_auth_relative_key_path() {
		// Relative key_path is pre-resolved by the config loader.
		// This test simulates the post-resolution state.
		let dir = tempfile::tempdir().unwrap();
		let key_file = dir.path().join("my_key");
		std::fs::write(&key_file, "fake key").unwrap();

		let config = config_with_ssh(SshConfig {
			key_path: Some(RelativePathBuf::from(&key_file)),
			..Default::default()
		});

		let result = resolve_auth(&config);
		assert!(result.is_ok());
	}

	#[test]
	fn test_resolve_default_key_path_no_config() {
		let _ = resolve_default_key_path();
	}

	#[test]
	fn test_try_agent_checks_env() {
		let _ = try_agent();
	}

	#[test]
	fn test_password_config_string() {
		let config = config_with_ssh(SshConfig {
			password: PasswordConfig::Value("secret".to_string()),
			..Default::default()
		});
		let result = resolve_auth(&config);
		assert!(result.is_ok());
	}

	#[test]
	fn test_password_config_false() {
		let config = config_with_ssh(SshConfig {
			password: PasswordConfig::Interactive(false),
			..Default::default()
		});
		let _ = resolve_auth(&config);
	}
}
