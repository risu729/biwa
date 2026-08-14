//! Resolution of effective SSH connection settings.

use crate::Result;
use crate::config::types::SshConfig;
use color_eyre::eyre::{WrapErr as _, bail};
use russh::keys::{PublicKey, load_public_key, load_secret_key};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// A fully resolved SSH connection target.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
	clippy::module_name_repetitions,
	reason = "the explicit name distinguishes the resolved target from config input"
)]
pub struct ResolvedSshTarget {
	/// Host or alias originally supplied to Biwa.
	pub lookup_host: String,
	/// Network hostname after applying OpenSSH `HostName`.
	pub hostname: String,
	/// Effective remote port.
	pub port: u16,
	/// Effective remote username.
	pub user: String,
	/// Ordered OpenSSH `IdentityFile` entries.
	pub identity_files: Vec<PathBuf>,
}

impl ResolvedSshTarget {
	/// Resolves a target using the current user's OpenSSH configuration path.
	pub fn resolve(config: &SshConfig) -> Result<Self> {
		let ssh_config_path = homedir::my_home()
			.ok()
			.flatten()
			.map(|home| home.join(".ssh/config"));
		Self::resolve_with_path(config, ssh_config_path.as_deref())
	}

	/// Resolves a target with an explicit OpenSSH configuration path.
	pub(crate) fn resolve_with_path(
		config: &SshConfig,
		ssh_config_path: Option<&Path>,
	) -> Result<Self> {
		let openssh = if config.use_ssh_config {
			ssh_config_path
				.map(|path| parse_openssh(path, &config.host))
				.transpose()?
				.flatten()
		} else {
			None
		};

		let openssh_host = openssh.as_ref().map(|parsed| &parsed.host_config);
		let openssh_user = openssh_host.and_then(|host| host.user.as_deref());
		let user = reconcile_user(config.user.as_deref(), openssh_user, &config.host)?;
		let port = reconcile_port(
			config.port,
			openssh_host.and_then(|host| host.port),
			&config.host,
		)?;
		let identity_files = openssh_host
			.and_then(|host| host.identity_file.clone())
			.unwrap_or_default();

		if let Some(key_path) = config.key_path.as_deref() {
			reconcile_identity(key_path, &identity_files, &config.host)?;
		}

		Ok(Self {
			lookup_host: config.host.clone(),
			hostname: openssh
				.as_ref()
				.and_then(|parsed| parsed.host_config.hostname.clone())
				.unwrap_or_else(|| config.host.clone()),
			port,
			user,
			identity_files,
		})
	}
}

/// Parses the matching OpenSSH host block, treating a missing config as empty.
fn parse_openssh(path: &Path, host: &str) -> Result<Option<russh_config::Config>> {
	match russh_config::parse_path(path, host) {
		Ok(config) => Ok(Some(config)),
		Err(russh_config::Error::Io(error)) if error.kind() == ErrorKind::NotFound => Ok(None),
		Err(error) => Err(error).wrap_err_with(|| {
			format!(
				"Failed to parse OpenSSH configuration at {}. Fix the file or set ssh.use_ssh_config = false",
				path.display()
			)
		}),
	}
}

/// Reconciles the Biwa and OpenSSH username values.
fn reconcile_user(biwa: Option<&str>, openssh: Option<&str>, host: &str) -> Result<String> {
	match (biwa, openssh) {
		(Some(biwa), Some(openssh)) if biwa != openssh => bail!(
			"Conflicting SSH user configuration for `{host}`: Biwa specifies `{biwa}`, but ~/.ssh/config specifies `{openssh}`. Remove one setting or make them match"
		),
		(Some(user), _) | (None, Some(user)) => Ok(user.to_owned()),
		(None, None) => bail!(
			"Missing SSH user for `{host}`. Set ssh.user (or BIWA_SSH_USER), or add User to the matching ~/.ssh/config entry"
		),
	}
}

/// Reconciles the Biwa and OpenSSH port values.
fn reconcile_port(biwa: Option<u16>, openssh: Option<u16>, host: &str) -> Result<u16> {
	match (biwa, openssh) {
		(Some(biwa), Some(openssh)) if biwa != openssh => bail!(
			"Conflicting SSH port configuration for `{host}`: Biwa specifies `{biwa}`, but ~/.ssh/config specifies `{openssh}`. Remove one setting or make them match"
		),
		(Some(port), _) | (None, Some(port)) => Ok(port),
		(None, None) => Ok(22),
	}
}

/// Ensures duplicate Biwa and OpenSSH identity settings select the same key.
fn reconcile_identity(biwa: &Path, openssh: &[PathBuf], host: &str) -> Result<()> {
	if openssh.is_empty() {
		return Ok(());
	}
	if openssh.len() != 1 {
		bail!(
			"Conflicting SSH identity configuration for `{host}`: ssh.key_path is singular, but ~/.ssh/config supplies {} IdentityFile entries. Remove one source or configure exactly one equivalent identity",
			openssh.len()
		);
	}

	let biwa_key = public_key_for_identity(biwa).wrap_err_with(|| {
		format!(
			"Cannot compare ssh.key_path `{}` with OpenSSH IdentityFile",
			biwa.display()
		)
	})?;
	let openssh_path = openssh.first().expect("length checked above");
	let openssh_key = public_key_for_identity(openssh_path).wrap_err_with(|| {
		format!(
			"Cannot compare OpenSSH IdentityFile `{}` with ssh.key_path",
			openssh_path.display()
		)
	})?;

	if biwa_key.key_data() != openssh_key.key_data() {
		bail!(
			"Conflicting SSH identity configuration for `{host}`: ssh.key_path `{}` and OpenSSH IdentityFile `{}` contain different public keys",
			biwa.display(),
			openssh_path.display()
		);
	}
	Ok(())
}

/// Loads public key material from a public key, companion file, or private key.
fn public_key_for_identity(path: &Path) -> Result<PublicKey> {
	if let Ok(key) = load_public_key(path) {
		return Ok(key);
	}

	let companion = PathBuf::from(format!("{}.pub", path.to_string_lossy()));
	if let Ok(key) = load_public_key(&companion) {
		return Ok(key);
	}

	let private = load_secret_key(path, None).wrap_err_with(|| {
		format!(
			"Failed to read public key material from `{}` or `{}`",
			path.display(),
			companion.display()
		)
	})?;
	Ok(private.public_key().clone())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::types::Config;
	use pretty_assertions::assert_eq;
	use std::fs;
	use tempfile::tempdir;

	const PUBLIC_KEY_A: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGqfEeyNrOxuH87ZVirsvRm72W3vrW3qJKbBqjsoKn3Z biwa-e2e";
	const PUBLIC_KEY_B: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIWBVymg7tyFs+jzE07UpfXkQEibpPg23d2KCVnIvxLN biwa-other";

	fn config(host: &str) -> SshConfig {
		let mut ssh = Config::default().ssh;
		ssh.host = host.to_owned();
		ssh.user = None;
		ssh.port = None;
		ssh
	}

	#[test]
	fn resolves_openssh_alias() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("config");
		fs::write(
			&path,
			"Host cse\n  HostName login.cse.unsw.edu.au\n  User z1234567\n  Port 2222\n  IdentityFile ~/.ssh/cse.pub\n",
		)?;

		let target = ResolvedSshTarget::resolve_with_path(&config("cse"), Some(&path))?;
		assert_eq!(target.lookup_host, "cse");
		assert_eq!(target.hostname, "login.cse.unsw.edu.au");
		assert_eq!(target.user, "z1234567");
		assert_eq!(target.port, 2222);
		assert_eq!(target.identity_files.len(), 1);
		Ok(())
	}

	#[test]
	fn accepts_matching_values() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("config");
		fs::write(&path, "Host cse\n  User alice\n  Port 22\n")?;
		let mut ssh = config("cse");
		ssh.user = Some("alice".to_owned());
		ssh.port = Some(22);

		let target = ResolvedSshTarget::resolve_with_path(&ssh, Some(&path))?;
		assert_eq!(target.user, "alice");
		assert_eq!(target.port, 22);
		Ok(())
	}

	#[test]
	fn rejects_conflicting_user() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("config");
		fs::write(&path, "Host cse\n  User bob\n")?;
		let mut ssh = config("cse");
		ssh.user = Some("alice".to_owned());

		let error = ResolvedSshTarget::resolve_with_path(&ssh, Some(&path))
			.expect_err("different users conflict");
		assert!(error.to_string().contains("Conflicting SSH user"));
		Ok(())
	}

	#[test]
	fn disabled_openssh_config_uses_biwa_values() -> Result<()> {
		let mut ssh = config("example.test");
		ssh.user = Some("alice".to_owned());
		ssh.use_ssh_config = false;
		let target = ResolvedSshTarget::resolve_with_path(&ssh, Some(Path::new("missing")))?;
		assert_eq!(target.hostname, "example.test");
		assert_eq!(target.port, 22);
		Ok(())
	}

	#[test]
	fn resolve_uses_direct_values_when_openssh_is_disabled() -> Result<()> {
		let mut ssh = config("example.test");
		ssh.user = Some("alice".to_owned());
		ssh.key_path = Some(PathBuf::from("unused-key"));
		ssh.use_ssh_config = false;

		let target = ResolvedSshTarget::resolve(&ssh)?;
		assert_eq!(target.hostname, "example.test");
		assert_eq!(target.user, "alice");
		assert_eq!(target.port, 22);
		Ok(())
	}

	#[test]
	fn missing_openssh_config_uses_biwa_values() -> Result<()> {
		let dir = tempdir()?;
		let mut ssh = config("example.test");
		ssh.user = Some("alice".to_owned());
		let target =
			ResolvedSshTarget::resolve_with_path(&ssh, Some(&dir.path().join("missing-config")))?;

		assert_eq!(target.hostname, "example.test");
		assert_eq!(target.user, "alice");
		assert_eq!(target.port, 22);
		assert!(target.identity_files.is_empty());
		Ok(())
	}

	#[test]
	fn missing_user_reports_how_to_configure_it() {
		let error = ResolvedSshTarget::resolve_with_path(&config("example.test"), None)
			.expect_err("a user is required");

		assert!(error.to_string().contains("Missing SSH user"));
	}

	#[test]
	fn rejects_conflicting_port() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("config");
		fs::write(&path, "Host cse\n  User alice\n  Port 2222\n")?;
		let mut ssh = config("cse");
		ssh.user = Some("alice".to_owned());
		ssh.port = Some(22);

		let error = ResolvedSshTarget::resolve_with_path(&ssh, Some(&path))
			.expect_err("different ports conflict");
		assert!(error.to_string().contains("Conflicting SSH port"));
		Ok(())
	}

	#[test]
	fn invalid_openssh_config_has_actionable_context() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("config");
		fs::write(&path, "User alice\nHost cse\n")?;

		let error = ResolvedSshTarget::resolve_with_path(&config("cse"), Some(&path))
			.expect_err("settings before the first Host block must not be ignored");
		let message = error.to_string();
		assert!(message.contains("Failed to parse OpenSSH configuration"));
		assert!(message.contains("ssh.use_ssh_config = false"));
		Ok(())
	}

	#[test]
	fn rejects_multiple_openssh_identities_with_biwa_key() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("config");
		fs::write(
			&path,
			"Host cse\n  User alice\n  IdentityFile first\n  IdentityFile second\n",
		)?;
		let mut ssh = config("cse");
		ssh.key_path = Some(dir.path().join("biwa-key"));

		let error = ResolvedSshTarget::resolve_with_path(&ssh, Some(&path))
			.expect_err("one Biwa key cannot match multiple OpenSSH identities");
		assert!(
			error
				.to_string()
				.contains("supplies 2 IdentityFile entries")
		);
		Ok(())
	}

	#[test]
	fn accepts_matching_public_identity_files() -> Result<()> {
		let dir = tempdir()?;
		let biwa_key = dir.path().join("biwa.pub");
		let openssh_key = dir.path().join("openssh.pub");
		fs::write(&biwa_key, PUBLIC_KEY_A)?;
		fs::write(&openssh_key, PUBLIC_KEY_A)?;
		let config_path = dir.path().join("config");
		fs::write(
			&config_path,
			format!(
				"Host cse\n  User alice\n  IdentityFile {}\n",
				openssh_key.display()
			),
		)?;
		let mut ssh = config("cse");
		ssh.key_path = Some(biwa_key);

		let target = ResolvedSshTarget::resolve_with_path(&ssh, Some(&config_path))?;
		assert_eq!(target.identity_files, vec![openssh_key]);
		Ok(())
	}

	#[test]
	fn accepts_companion_public_key() -> Result<()> {
		let dir = tempdir()?;
		let biwa_key = dir.path().join("biwa");
		fs::write(biwa_key.with_extension("pub"), PUBLIC_KEY_A)?;
		let openssh_key = dir.path().join("openssh.pub");
		fs::write(&openssh_key, PUBLIC_KEY_A)?;
		let config_path = dir.path().join("config");
		fs::write(
			&config_path,
			format!(
				"Host cse\n  User alice\n  IdentityFile {}\n",
				openssh_key.display()
			),
		)?;
		let mut ssh = config("cse");
		ssh.key_path = Some(biwa_key);

		ResolvedSshTarget::resolve_with_path(&ssh, Some(&config_path))?;
		Ok(())
	}

	#[test]
	fn rejects_different_identity_keys() -> Result<()> {
		let dir = tempdir()?;
		let biwa_key = dir.path().join("biwa.pub");
		let openssh_key = dir.path().join("openssh.pub");
		fs::write(&biwa_key, PUBLIC_KEY_A)?;
		fs::write(&openssh_key, PUBLIC_KEY_B)?;
		let config_path = dir.path().join("config");
		fs::write(
			&config_path,
			format!(
				"Host cse\n  User alice\n  IdentityFile {}\n",
				openssh_key.display()
			),
		)?;
		let mut ssh = config("cse");
		ssh.key_path = Some(biwa_key);

		let error = ResolvedSshTarget::resolve_with_path(&ssh, Some(&config_path))
			.expect_err("different public keys conflict");
		assert!(error.to_string().contains("contain different public keys"));
		Ok(())
	}

	#[test]
	fn unreadable_biwa_identity_has_comparison_context() -> Result<()> {
		let dir = tempdir()?;
		let openssh_key = dir.path().join("openssh.pub");
		fs::write(&openssh_key, PUBLIC_KEY_A)?;
		let config_path = dir.path().join("config");
		fs::write(
			&config_path,
			format!(
				"Host cse\n  User alice\n  IdentityFile {}\n",
				openssh_key.display()
			),
		)?;
		let mut ssh = config("cse");
		ssh.key_path = Some(dir.path().join("missing"));

		let error = ResolvedSshTarget::resolve_with_path(&ssh, Some(&config_path))
			.expect_err("missing key material cannot be compared");
		assert!(error.to_string().contains("Cannot compare ssh.key_path"));
		Ok(())
	}

	#[test]
	fn unreadable_openssh_identity_has_comparison_context() -> Result<()> {
		let dir = tempdir()?;
		let biwa_key = dir.path().join("biwa.pub");
		fs::write(&biwa_key, PUBLIC_KEY_A)?;
		let missing_openssh_key = dir.path().join("missing.pub");
		let config_path = dir.path().join("config");
		fs::write(
			&config_path,
			format!(
				"Host cse\n  User alice\n  IdentityFile {}\n",
				missing_openssh_key.display()
			),
		)?;
		let mut ssh = config("cse");
		ssh.key_path = Some(biwa_key);

		let error = ResolvedSshTarget::resolve_with_path(&ssh, Some(&config_path))
			.expect_err("missing OpenSSH key material cannot be compared");
		assert!(
			error
				.to_string()
				.contains("Cannot compare OpenSSH IdentityFile")
		);
		Ok(())
	}
}
