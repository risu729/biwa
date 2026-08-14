/// Authentication types.
pub mod auth;
/// Command execution.
pub mod execute;

use self::auth::{Method, authenticate};
use self::execute::execute;

use crate::Result;
use crate::config::types::HostKeyChecking;
use alloc::sync::Arc;
use color_eyre::eyre::{Context as _, Report};
use core::fmt;
use core::fmt::Debug;
use core::future::{Future, ready};
use core::result::Result as CoreResult;
use hmac::{Hmac, KeyInit as _, Mac as _};
use russh::Channel;
use russh::client::{Config, Handle, Handler, Msg, connect as russh_connect};
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::ssh_key::known_hosts::{Entry, HostPatterns, Marker};
use russh::keys::{Error as KeyError, PublicKey, PublicKeyOrCertificate};
use sha1::Sha1;
use std::error::Error as StdError;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::ToSocketAddrs;
use tokio::net::lookup_host;

/// Whether the insecure host-key warning has already been emitted.
static INSECURE_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

/// Structured marker for errors that must stop before authentication or retry.
#[derive(Debug)]
pub struct HostKeyVerificationFailed {
	/// Actionable verification failure message.
	message: String,
}

impl HostKeyVerificationFailed {
	/// Creates a structured host-key verification failure.
	pub(crate) fn new(message: impl Into<String>) -> Self {
		Self {
			message: message.into(),
		}
	}
}

impl fmt::Display for HostKeyVerificationFailed {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.message)
	}
}

impl StdError for HostKeyVerificationFailed {}

/// Host-key verification settings for one resolved target.
#[derive(Debug, Clone)]
pub struct HostKeyVerification {
	/// Network hostname used in known-hosts entries.
	hostname: String,
	/// Network port used in known-hosts entries.
	port: u16,
	/// Verification policy.
	checking: HostKeyChecking,
	/// Optional nonstandard known-hosts path.
	known_hosts: Option<PathBuf>,
}

impl HostKeyVerification {
	/// Creates verification settings for a resolved target.
	#[must_use]
	pub const fn new(
		hostname: String,
		port: u16,
		checking: HostKeyChecking,
		known_hosts: Option<PathBuf>,
	) -> Self {
		Self {
			hostname,
			port,
			checking,
			known_hosts,
		}
	}

	/// Resolves the known-hosts path used by every verification operation.
	fn path(&self) -> CoreResult<PathBuf, KeyError> {
		self.known_hosts.as_ref().map_or_else(
			|| {
				homedir::my_home()
					.ok()
					.flatten()
					.map(|home| home.join(".ssh/known_hosts"))
					.ok_or(KeyError::NoHomeDir)
			},
			|path| Ok(path.clone()),
		)
	}

	/// Checks whether the key is already trusted.
	fn check(&self, key: &PublicKey) -> CoreResult<bool, KeyError> {
		check_known_hosts_path(&self.hostname, self.port, key, self.path()?)
	}

	/// Records a newly trusted key.
	fn learn(&self, key: &PublicKey) -> CoreResult<(), KeyError> {
		learn_known_hosts_path(&self.hostname, self.port, key, self.path()?)
	}

	/// Checks whether the presented key is explicitly revoked for this host.
	fn is_revoked(&self, key: &PublicKey) -> CoreResult<bool, KeyError> {
		let contents = match fs::read_to_string(self.path()?) {
			Ok(contents) => contents,
			Err(error)
				if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
			{
				return Ok(false);
			}
			Err(error) => return Err(KeyError::IO(error)),
		};
		let host = if self.port == 22 {
			self.hostname.clone()
		} else {
			format!("[{}]:{}", self.hostname, self.port)
		};

		for line in contents.lines().map(str::trim) {
			if !line.starts_with("@revoked ") {
				continue;
			}
			let entry = line.parse::<Entry>().map_err(KeyError::from)?;
			if entry.marker() == Some(&Marker::Revoked)
				&& entry.public_key().key_data() == key.key_data()
				&& host_patterns_match(entry.host_patterns(), &host)
			{
				return Ok(true);
			}
		}
		Ok(false)
	}

	/// Describes the known-hosts source without assuming a home directory exists.
	fn source(&self) -> String {
		self.known_hosts.as_deref().map_or_else(
			|| "the default ~/.ssh/known_hosts file".to_owned(),
			|path| format!("`{}`", path.display()),
		)
	}

	/// Wraps a known-hosts error in the structured terminal-failure marker.
	#[expect(
		clippy::wildcard_enum_match_arm,
		reason = "dependency error variants other than changed keys share one diagnostic"
	)]
	fn check_error(&self, error: &KeyError) -> Report {
		let message = match error {
			KeyError::KeyChanged { line } => format!(
				"SSH host key for {}:{} changed ({} line {line}). Refusing to connect; verify the server and remove the stale entry if the change is legitimate",
				self.hostname,
				self.port,
				self.source()
			),
			_ => format!(
				"Failed to verify SSH host key for {}:{} using {}: {error}",
				self.hostname,
				self.port,
				self.source()
			),
		};
		Report::new(HostKeyVerificationFailed::new(message))
	}
}

/// Handler for the SSH client.
struct ClientHandler {
	/// Host-key settings for this connection.
	verification: HostKeyVerification,
}

impl ClientHandler {
	/// Verifies a server key using the configured host-key policy.
	fn verify_server_key(
		&mut self,
		server_key: &PublicKeyOrCertificate,
	) -> CoreResult<bool, Report> {
		if self.verification.checking == HostKeyChecking::Insecure {
			if !INSECURE_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
				tracing::warn!(
					host = %self.verification.hostname,
					port = self.verification.port,
					"SSH host key verification is disabled"
				);
			}
			return Ok(true);
		}

		let PublicKeyOrCertificate::PublicKey {
			key: server_public_key,
			..
		} = server_key
		else {
			return Err(Report::new(HostKeyVerificationFailed::new(format!(
				"SSH host certificates are not supported for {}:{}. Refusing to connect",
				self.verification.hostname, self.verification.port
			))));
		};

		match self.verification.is_revoked(server_public_key) {
			Ok(true) => {
				return Err(Report::new(HostKeyVerificationFailed::new(format!(
					"SSH host key for {}:{} is explicitly revoked in {}",
					self.verification.hostname,
					self.verification.port,
					self.verification.source()
				))));
			}
			Ok(false) => {}
			Err(error) => return Err(self.verification.check_error(&error)),
		}

		match self.verification.check(server_public_key) {
			Ok(true) => Ok(true),
			Ok(false) if self.verification.checking == HostKeyChecking::AcceptNew => {
				self.verification
					.learn(server_public_key)
					.map_err(|error| {
						Report::new(HostKeyVerificationFailed::new(format!(
							"Failed to record SSH host key for {}:{} in {}: {error}",
							self.verification.hostname,
							self.verification.port,
							self.verification.source()
						)))
					})?;
				tracing::info!(
					host = %self.verification.hostname,
					port = self.verification.port,
					"Recorded new SSH host key"
				);
				Ok(true)
			}
			Ok(false) => Err(Report::new(HostKeyVerificationFailed::new(format!(
				"Unknown SSH host key for {}:{} in {}. Verify and add the key, or set ssh.host_key_checking = \"accept-new\" for trust on first use",
				self.verification.hostname,
				self.verification.port,
				self.verification.source()
			)))),
			Err(error) => Err(self.verification.check_error(&error)),
		}
	}
}

impl Handler for ClientHandler {
	type Error = Report;

	fn check_server_key(
		&mut self,
		server_public_key: &PublicKeyOrCertificate,
	) -> impl Future<Output = CoreResult<bool, Self::Error>> {
		ready(self.verify_server_key(server_public_key))
	}
}

/// Matches parsed known-host patterns against the effective host and port.
fn host_patterns_match(patterns: &HostPatterns, host: &str) -> bool {
	match patterns {
		HostPatterns::Patterns(patterns) => {
			let mut positive_match = false;
			for pattern in patterns {
				let (negated, pattern) = pattern
					.strip_prefix('!')
					.map_or((false, pattern.as_str()), |pattern| (true, pattern));
				if wildcard_match(pattern, host) {
					if negated {
						return false;
					}
					positive_match = true;
				}
			}
			positive_match
		}
		HostPatterns::HashedName { salt, hash } => {
			Hmac::<Sha1>::new_from_slice(salt).is_ok_and(|hmac| {
				hmac.chain_update(host.as_bytes())
					.verify_slice(hash)
					.is_ok()
			})
		}
	}
}

/// Matches OpenSSH's `*` and `?` host wildcards case-insensitively.
#[expect(
	clippy::arithmetic_side_effects,
	clippy::indexing_slicing,
	reason = "each index operation is guarded by explicit monotonic length checks"
)]
fn wildcard_match(pattern: &str, value: &str) -> bool {
	let pattern = pattern.as_bytes();
	let value = value.as_bytes();
	let (mut pattern_index, mut value_index) = (0, 0);
	let (mut star_index, mut star_value_index) = (None, 0);

	while value_index < value.len() {
		if pattern_index < pattern.len()
			&& (pattern[pattern_index] == b'?'
				|| pattern[pattern_index].eq_ignore_ascii_case(&value[value_index]))
		{
			pattern_index += 1;
			value_index += 1;
		} else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
			star_index = Some(pattern_index);
			pattern_index += 1;
			star_value_index = value_index;
		} else if let Some(star) = star_index {
			pattern_index = star + 1;
			star_value_index += 1;
			value_index = star_value_index;
		} else {
			return false;
		}
	}

	while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
		pattern_index += 1;
	}
	pattern_index == pattern.len()
}
/// An SSH client.
#[derive(Clone)]
pub struct Client {
	/// The active SSH connection handle.
	connection_handle: Arc<Handle<ClientHandler>>,
}

impl Client {
	/// Connect to a remote SSH server.
	pub async fn connect<T: ToSocketAddrs + Debug + Send + Sync>(
		addr: T,
		username: &str,
		auth: Method,
		verification: HostKeyVerification,
	) -> Result<Self> {
		let config = Arc::new(Config::default());

		let socket_addrs = lookup_host(&addr)
			.await
			.wrap_err("Failed to resolve addresses")?;

		let mut connect_res = None;
		let mut last_err: Option<Report> = None;

		for socket_addr in socket_addrs {
			let handler = ClientHandler {
				verification: verification.clone(),
			};
			match russh_connect(Arc::clone(&config), socket_addr, handler).await {
				Ok(h) => {
					connect_res = Some(h);
					break;
				}
				Err(e) => {
					if e.downcast_ref::<HostKeyVerificationFailed>().is_some() {
						return Err(e);
					}
					tracing::debug!(error = %e, %socket_addr, "Connection failed, trying next address");
					last_err = Some(e);
				}
			}
		}

		let Some(mut handle) = connect_res else {
			match last_err {
				Some(err) => {
					return Err(
						err.wrap_err(format!("Could not connect to any address for {addr:?}"))
					);
				}
				None => {
					return Err(color_eyre::eyre::eyre!(
						"Could not connect: no addresses resolved for {addr:?}"
					));
				}
			}
		};

		let username = username.to_owned();

		authenticate(&mut handle, &username, auth).await?;

		Ok(Self {
			connection_handle: Arc::new(handle),
		})
	}

	/// Open a new SSH channel.
	pub async fn get_channel(&self) -> Result<Channel<Msg>> {
		self.connection_handle
			.channel_open_session()
			.await
			.wrap_err("Failed to open channel")
	}

	/// Execute a command and collect its stdout, stderr, and exit status.
	pub async fn execute(&self, command: &str) -> Result<execute::CommandExecutedResult> {
		let mut channel = self.get_channel().await?;
		execute(&mut channel, command).await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use std::fs;
	use tempfile::tempdir;

	const KEY_ONE: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T";
	const KEY_TWO: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICQvZPc4//KJ9jQpzY7zfzt6isLmdQidqqeK4cdA0hY/";

	/// Parses one of the static test keys.
	fn key(value: &str) -> PublicKey {
		PublicKey::from_openssh(value).expect("static public key is valid")
	}

	/// Creates a handler backed by an isolated known-hosts file.
	fn handler(path: PathBuf, checking: HostKeyChecking, port: u16) -> ClientHandler {
		ClientHandler {
			verification: HostKeyVerification::new(
				"example.test".to_owned(),
				port,
				checking,
				Some(path),
			),
		}
	}

	#[tokio::test]
	async fn strict_accepts_matching_key() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("known_hosts");
		learn_known_hosts_path("example.test", 22, &key(KEY_ONE), &path)?;
		let accepted = handler(path, HostKeyChecking::Strict, 22)
			.check_server_key(&key(KEY_ONE))
			.await?;
		assert!(accepted);
		Ok(())
	}

	#[tokio::test]
	async fn strict_rejects_unknown_key() -> Result<()> {
		let dir = tempdir()?;
		let error = handler(dir.path().join("known_hosts"), HostKeyChecking::Strict, 22)
			.check_server_key(&key(KEY_ONE))
			.await
			.expect_err("strict mode rejects an unknown key");
		assert!(error.to_string().contains("Unknown SSH host key"));
		Ok(())
	}

	#[tokio::test]
	async fn changed_key_is_always_rejected() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("known_hosts");
		learn_known_hosts_path("example.test", 22, &key(KEY_ONE), &path)?;
		let error = handler(path, HostKeyChecking::AcceptNew, 22)
			.check_server_key(&key(KEY_TWO))
			.await
			.expect_err("accept-new must reject a changed key");
		assert!(error.to_string().contains("changed"));
		Ok(())
	}

	#[tokio::test]
	async fn accept_new_rejects_explicitly_revoked_key() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("known_hosts");
		let contents = format!("@revoked example.test {KEY_ONE}\n");
		fs::write(&path, &contents)?;

		let error = handler(path.clone(), HostKeyChecking::AcceptNew, 22)
			.check_server_key(&key(KEY_ONE))
			.await
			.expect_err("accept-new must not relearn a revoked key");

		assert!(error.to_string().contains("explicitly revoked"));
		assert_eq!(fs::read_to_string(path)?, contents);
		Ok(())
	}

	#[tokio::test]
	async fn revoked_hashed_nonstandard_host_is_rejected() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("known_hosts");
		fs::write(
			&path,
			format!(
				"@revoked |1|MDEyMzQ1Njc4OTAxMjM0NTY3ODk=|410uIDcPGPGynJ2kieNHiAb2uZY= {KEY_ONE}\n"
			),
		)?;

		let error = handler(path, HostKeyChecking::AcceptNew, 2222)
			.check_server_key(&key(KEY_ONE))
			.await
			.expect_err("hashed host-and-port entries must retain revoked semantics");

		assert!(error.to_string().contains("explicitly revoked"));
		Ok(())
	}

	#[test]
	fn revoked_host_patterns_support_wildcards_negation_and_case() -> Result<()> {
		let patterns = "*.example.test,!safe.example.test".parse::<HostPatterns>()?;

		assert!(host_patterns_match(&patterns, "build.EXAMPLE.test"));
		assert!(!host_patterns_match(&patterns, "safe.example.test"));
		assert!(!host_patterns_match(&patterns, "example.org"));
		assert!(wildcard_match("host?.example.test", "HOST1.example.test"));
		assert!(!wildcard_match("host?.example.test", "host12.example.test"));
		Ok(())
	}

	#[tokio::test]
	async fn revoked_key_for_another_host_does_not_block_accept_new() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("known_hosts");
		fs::write(&path, format!("@revoked other.example.test {KEY_ONE}\n"))?;

		let accepted = handler(path.clone(), HostKeyChecking::AcceptNew, 22)
			.check_server_key(&key(KEY_ONE))
			.await?;

		assert!(accepted);
		assert!(check_known_hosts_path(
			"example.test",
			22,
			&key(KEY_ONE),
			path
		)?);
		Ok(())
	}

	#[tokio::test]
	async fn accept_new_records_nonstandard_port() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("nested/known_hosts");
		let accepted = handler(path.clone(), HostKeyChecking::AcceptNew, 2222)
			.check_server_key(&key(KEY_ONE))
			.await?;
		assert!(accepted);
		let contents = fs::read_to_string(&path)?;
		assert_eq!(
			contents.split_whitespace().next(),
			Some("[example.test]:2222")
		);
		assert!(check_known_hosts_path(
			"example.test",
			2222,
			&key(KEY_ONE),
			path
		)?);
		Ok(())
	}

	#[tokio::test]
	async fn insecure_accepts_unknown_key() -> Result<()> {
		let dir = tempdir()?;
		let accepted = handler(
			dir.path().join("known_hosts"),
			HostKeyChecking::Insecure,
			22,
		)
		.check_server_key(&key(KEY_ONE))
		.await?;

		assert!(accepted);
		Ok(())
	}

	#[tokio::test]
	async fn accept_new_reports_recording_failure() -> Result<()> {
		let dir = tempdir()?;
		let parent = dir.path().join("not-a-directory");
		fs::write(&parent, "regular file")?;
		let path = parent.join("known_hosts");

		let error = handler(path, HostKeyChecking::AcceptNew, 22)
			.check_server_key(&key(KEY_ONE))
			.await
			.expect_err("a file cannot be used as the known-hosts parent directory");
		assert!(error.to_string().contains("Failed to record SSH host key"));
		Ok(())
	}

	#[tokio::test]
	async fn strict_reports_known_hosts_read_failure() -> Result<()> {
		let dir = tempdir()?;
		let path = dir.path().join("known_hosts-directory");
		fs::create_dir_all(&path)?;

		let error = handler(path, HostKeyChecking::Strict, 22)
			.check_server_key(&key(KEY_ONE))
			.await
			.expect_err("a directory cannot be parsed as known-hosts data");
		assert!(error.to_string().contains("Failed to verify SSH host key"));
		Ok(())
	}

	#[test]
	fn default_known_hosts_source_is_descriptive() {
		let verification =
			HostKeyVerification::new("example.test".to_owned(), 22, HostKeyChecking::Strict, None);

		assert_eq!(verification.source(), "the default ~/.ssh/known_hosts file");
	}
}
