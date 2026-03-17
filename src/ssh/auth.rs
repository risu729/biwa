use crate::Result;
use crate::config::types::Config;
use crate::config::types::PasswordConfig;
use async_ssh2_tokio::client::AuthMethod;
use color_eyre::eyre::bail;
use dialoguer::Password;
use russh::keys::{Error as RusshKeysError, load_secret_key};
use std::env;
use std::path::{Path, PathBuf};
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
pub(super) fn resolve_auth(config: &Config) -> Result<AuthMethod> {
	let ssh = &config.ssh;

	// 1. Explicit key_path (paths are already resolved natively by confique)
	if let Some(path) = &ssh.key_path {
		if path.exists() {
			info!(path = %path.display(), "Using configured SSH key file");
			return load_key(path);
		}
		bail!("Configured SSH key file not found: {}", path.display());
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
		return load_key(&key_path);
	}

	// 4. SSH Agent as last resort (for zero-config users)
	if try_agent(env::var("SSH_AUTH_SOCK").ok().as_deref()) {
		info!("Using SSH agent authentication");
		return Ok(AuthMethod::with_agent());
	}

	bail!(
		"No authentication method available. \
		Configure ssh.key_path, ssh.password, or set up an SSH agent."
	)
}

/// Load an SSH key from the given path, prompting for a passphrase if needed.
fn load_key(path: &Path) -> Result<AuthMethod> {
	let path_str = path.to_string_lossy();
	match load_secret_key(path_str.as_ref(), None) {
		Err(RusshKeysError::KeyIsEncrypted) => {
			info!(path = %path.display(), "SSH key is encrypted, prompting for passphrase");
			let passphrase = Password::new()
				.with_prompt(format!("Passphrase for {}", path.display()))
				.interact()?;
			Ok(AuthMethod::with_key_file(
				path_str.as_ref(),
				Some(&passphrase),
			))
		}
		_ => {
			// If it succeeds, or fails with any other error (e.g. invalid format),
			// we let the actual SSH connection attempt handle it and report the error.
			Ok(AuthMethod::with_key_file(path_str.as_ref(), None))
		}
	}
}

/// Determine whether SSH agent authentication should be used based on the provided socket path.
///
/// Callers typically pass the value of the `SSH_AUTH_SOCK` environment variable
/// (e.g. `std::env::var("SSH_AUTH_SOCK").ok().as_deref()`), and this function
/// simply checks whether that value is present and non-empty.
fn try_agent(auth_sock: Option<&str>) -> bool {
	match auth_sock {
		Some(sock) if !sock.is_empty() => {
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
		let expanded = default_path.strip_prefix("~/").map_or_else(
			|| PathBuf::from(default_path),
			|stripped| home.join(stripped),
		);

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
	use pretty_assertions::assert_matches;
	use serial_test::serial;
	use std::fs;

	#[serial]
	#[test]
	fn resolve_default_key_path_explicit() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let key_file = dir.path().join("my_key");
		fs::write(&key_file, "fake key")?;

		let mut config = Config::default();
		config.ssh.key_path = Some(key_file);

		let method = resolve_auth(&config)?;
		assert_matches!(method, AuthMethod::PrivateKeyFile { .. });
		Ok(())
	}

	#[serial]
	#[test]
	fn resolve_auth_missing_explicit_key_errors() {
		let mut config = Config::default();
		config.ssh.key_path = Some(PathBuf::from("/nonexistent/path/key"));

		let result = resolve_auth(&config);
		assert!(result.is_err());
		let err_msg = result.unwrap_err().to_string();
		assert!(err_msg.contains("not found"), "Error: {err_msg}");
	}

	#[serial]
	#[test]
	fn resolve_default_key_path_no_config() {
		// Verify the function runs without panic; it may or may not find a key
		// depending on the test environment.
		let _path = resolve_default_key_path();
	}

	#[test]
	fn try_agent_checks_env() {
		assert!(
			try_agent(Some("/tmp/fake-agent.sock")),
			"expected agent to be detected when SSH_AUTH_SOCK is set"
		);

		assert!(
			!try_agent(None),
			"expected no agent when SSH_AUTH_SOCK is unset"
		);
	}

	#[serial]
	#[test]
	fn password_config_string() -> Result<()> {
		let mut config = Config::default();
		config.ssh.password = PasswordConfig::Value("secret".to_owned());
		let method = resolve_auth(&config)?;
		assert_matches!(method, AuthMethod::Password(_));
		Ok(())
	}

	#[serial]
	#[test]
	fn password_config_false() {
		let mut config = Config::default();
		config.ssh.password = PasswordConfig::Interactive(false);
		let result = resolve_auth(&config);
		// Without explicit password, it may fall back to agent or key, or fail
		// — but it must not use Password auth (password = false means skip password)
		if let Ok(method) = result {
			assert_matches!(
				method,
				AuthMethod::PrivateKeyFile { .. } | AuthMethod::Agent
			);
		}
	}
}
