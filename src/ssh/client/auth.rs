use crate::Result;
use alloc::sync::Arc;
use color_eyre::eyre::Report;
use core::error::Error as StdError;
use core::fmt;
use dialoguer::Password;
use russh::client::{AuthResult, Handle, Handler};
use russh::keys::agent::{AgentIdentity, client::AgentClient};
use russh::keys::{
	Certificate, Error as KeyError, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey,
	load_secret_key,
};
use std::path::{Path, PathBuf};

/// Marker error for SSH authentication failures.
///
/// Returned from [`authenticate`] so callers can distinguish credential failures from transport
/// and host-key failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationFailureKind {
	/// The current credential failed locally or was rejected; another candidate may work.
	Retryable,
	/// Authentication must stop without trying another credential or transport.
	Terminal,
}

/// Structured marker for an authentication outcome and its retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationFailed {
	/// Whether credential fallback is permitted.
	kind: AuthenticationFailureKind,
}

impl AuthenticationFailed {
	/// Creates a failure that permits trying another credential.
	#[must_use]
	pub(crate) const fn retryable() -> Self {
		Self {
			kind: AuthenticationFailureKind::Retryable,
		}
	}

	/// Creates a failure that must stop authentication immediately.
	#[must_use]
	pub(crate) const fn terminal() -> Self {
		Self {
			kind: AuthenticationFailureKind::Terminal,
		}
	}

	/// Returns the retry policy attached to this authentication failure.
	#[must_use]
	pub const fn kind(self) -> AuthenticationFailureKind {
		self.kind
	}
}

impl fmt::Display for AuthenticationFailed {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("SSH authentication failed")
	}
}

impl StdError for AuthenticationFailed {}

/// One exact identity exposed by an SSH agent.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentCredential {
	/// Underlying public key used for fingerprints and `IdentityFile` hint matching.
	public_key: PublicKey,
	/// Full OpenSSH certificate when the agent identity is a certificate.
	certificate: Option<Certificate>,
}

impl AgentCredential {
	/// Creates a plain public-key agent credential.
	#[must_use]
	pub const fn from_public_key(public_key: PublicKey) -> Self {
		Self {
			public_key,
			certificate: None,
		}
	}

	/// Preserves the exact kind and wire identity returned by an SSH agent.
	#[must_use]
	pub fn from_identity(identity: AgentIdentity) -> Self {
		match identity {
			AgentIdentity::PublicKey { key, .. } => Self::from_public_key(key),
			AgentIdentity::Certificate { certificate, .. } => Self {
				public_key: certificate.public_key().clone().into(),
				certificate: Some(certificate),
			},
		}
	}

	/// Returns the underlying key for selector matching and display.
	#[must_use]
	pub const fn public_key(&self) -> &PublicKey {
		&self.public_key
	}

	/// Returns whether a freshly enumerated identity is this exact credential.
	fn matches_identity(&self, identity: &AgentIdentity) -> bool {
		match (&self.certificate, identity) {
			(None, AgentIdentity::PublicKey { key, .. }) => {
				self.public_key.key_data() == key.key_data()
			}
			(Some(expected), AgentIdentity::Certificate { certificate, .. }) => {
				expected == certificate
			}
			(None, AgentIdentity::Certificate { .. })
			| (Some(_), AgentIdentity::PublicKey { .. }) => false,
		}
	}

	/// Returns whether this identity is an OpenSSH certificate.
	const fn is_certificate(&self) -> bool {
		self.certificate.is_some()
	}
}

impl fmt::Debug for AgentCredential {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("AgentCredential")
			.field("fingerprint", &self.public_key.fingerprint(HashAlg::Sha256))
			.field("certificate", &self.is_certificate())
			.finish()
	}
}

/// One concrete credential to try on a fresh SSH connection.
#[derive(Clone, PartialEq, Eq)]
pub enum Method {
	/// Password authentication with an environment-provided secret.
	Password(String),
	/// Interactive password authentication explicitly requested by the user.
	PasswordPrompt {
		/// Prompt text that identifies the resolved account.
		prompt: String,
	},
	/// A specific local private key.
	PrivateKeyFile {
		/// Path to the private key file.
		key_file_path: PathBuf,
		/// Whether an encrypted key may prompt for its passphrase.
		allow_prompt: bool,
		/// Whether a local load/decryption failure may advance to another automatic candidate.
		fallback_on_load_failure: bool,
	},
	/// One specific identity exposed by the environment-selected SSH agent.
	AgentKey {
		/// Exact identity used to find the same agent entry again before signing.
		credential: Box<AgentCredential>,
	},
}

impl Method {
	/// Creates a password authentication method.
	pub fn with_password<S: Into<String>>(password: S) -> Self {
		Self::Password(password.into())
	}

	/// Creates an interactive password method.
	pub fn with_password_prompt<S: Into<String>>(prompt: S) -> Self {
		Self::PasswordPrompt {
			prompt: prompt.into(),
		}
	}

	/// Creates a private-key-file method.
	pub fn with_key_file<T: AsRef<Path>>(key_file_path: T, allow_prompt: bool) -> Self {
		Self::PrivateKeyFile {
			key_file_path: key_file_path.as_ref().to_path_buf(),
			allow_prompt,
			fallback_on_load_failure: false,
		}
	}

	/// Creates an automatically discovered private-key-file method.
	pub fn with_automatic_key_file<T: AsRef<Path>>(key_file_path: T, allow_prompt: bool) -> Self {
		Self::PrivateKeyFile {
			key_file_path: key_file_path.as_ref().to_path_buf(),
			allow_prompt,
			fallback_on_load_failure: true,
		}
	}

	/// Creates a method for one exact SSH-agent credential.
	pub fn with_agent_credential(credential: AgentCredential) -> Self {
		Self::AgentKey {
			credential: Box::new(credential),
		}
	}

	/// Returns a redacted description suitable for aggregated errors.
	#[must_use]
	pub fn description(&self) -> String {
		match self {
			Self::Password(_) | Self::PasswordPrompt { .. } => "password".to_owned(),
			Self::PrivateKeyFile { key_file_path, .. } => {
				format!("key {}", key_file_path.display())
			}
			Self::AgentKey { credential } => {
				let kind = if credential.is_certificate() {
					"agent certificate"
				} else {
					"agent key"
				};
				format!(
					"{kind} {}",
					credential.public_key.fingerprint(HashAlg::Sha256)
				)
			}
		}
	}
}

impl fmt::Debug for Method {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Password(_) => f.debug_tuple("Password").field(&"***").finish(),
			Self::PasswordPrompt { .. } => f.debug_struct("PasswordPrompt").finish(),
			Self::PrivateKeyFile {
				key_file_path,
				allow_prompt,
				fallback_on_load_failure,
			} => f
				.debug_struct("PrivateKeyFile")
				.field("key_file_path", key_file_path)
				.field("allow_prompt", allow_prompt)
				.field("fallback_on_load_failure", fallback_on_load_failure)
				.finish(),
			Self::AgentKey { credential } => f
				.debug_struct("AgentKey")
				.field("credential", credential)
				.finish(),
		}
	}
}

/// Creates a structured authentication failure with redacted context.
fn failure(context: impl fmt::Display + Send + Sync + 'static) -> Report {
	Report::from(AuthenticationFailed::retryable()).wrap_err(context)
}

/// Creates a terminal authentication failure that must not trigger fallback or transport retries.
fn terminal_failure(context: impl fmt::Display + Send + Sync + 'static) -> Report {
	Report::from(AuthenticationFailed::terminal()).wrap_err(context)
}

/// Converts a Russh authentication result into the supported single-factor contract.
fn require_success(result: &AuthResult, method: &str) -> Result<()> {
	match result {
		AuthResult::Success => Ok(()),
		AuthResult::Failure {
			partial_success: true,
			..
		} => Err(terminal_failure(
			"The server accepted one factor but requires additional authentication; MFA chains are not supported",
		)),
		AuthResult::Failure { .. } => Err(failure(format!("{method} authentication was rejected"))),
	}
}

/// Loads a private key, prompting only when this concrete candidate is reached and interaction is
/// allowed.
pub fn load_private_key(
	path: &Path,
	allow_prompt: bool,
	fallback_on_load_failure: bool,
) -> Result<(PrivateKey, bool)> {
	match load_secret_key(path, None) {
		Ok(key) => Ok((key, true)),
		Err(KeyError::KeyIsEncrypted) if allow_prompt => {
			let passphrase = Password::new()
				.with_prompt(format!("Passphrase for {}", path.display()))
				.interact()
				.map_err(|error| {
					terminal_failure(format!("Failed to read private-key passphrase: {error}"))
				})?;
			load_secret_key(path, Some(&passphrase))
				.map(|key| (key, false))
				.map_err(|error| {
					terminal_failure(format!(
						"Failed to decrypt SSH private key {}: {error}",
						path.display()
					))
				})
		}
		Err(KeyError::KeyIsEncrypted) => {
			let context = format!(
				"SSH private key {} is encrypted and interactive input is unavailable",
				path.display()
			);
			Err(if fallback_on_load_failure {
				failure(context)
			} else {
				terminal_failure(context)
			})
		}
		Err(error) => {
			let context = format!("Failed to load SSH private key {}: {error}", path.display());
			Err(if fallback_on_load_failure {
				failure(context)
			} else {
				terminal_failure(context)
			})
		}
	}
}

/// Authenticates a connected handle using exactly one concrete credential.
pub(super) async fn authenticate<H: Handler>(
	handle: &mut Handle<H>,
	username: &str,
	auth: Method,
) -> Result<bool> {
	match auth {
		Method::Password(password) => {
			let result = handle
				.authenticate_password(username, password)
				.await
				.map_err(|error| {
					terminal_failure(format!("Password authentication exchange failed: {error}"))
				})?;
			require_success(&result, "Password")?;
			Ok(true)
		}
		Method::PasswordPrompt { prompt } => {
			let password = Password::new()
				.with_prompt(prompt)
				.interact()
				.map_err(|error| terminal_failure(format!("Failed to read password: {error}")))?;
			let result = handle
				.authenticate_password(username, password)
				.await
				.map_err(|error| {
					terminal_failure(format!("Password authentication exchange failed: {error}"))
				})?;
			require_success(&result, "Password")?;
			Ok(false)
		}
		Method::PrivateKeyFile {
			key_file_path,
			allow_prompt,
			fallback_on_load_failure,
		} => {
			let (private_key, reusable_noninteractively) =
				load_private_key(&key_file_path, allow_prompt, fallback_on_load_failure)?;
			let hash_alg = handle
				.best_supported_rsa_hash()
				.await
				.map_err(|error| {
					terminal_failure(format!("Failed to negotiate key algorithm: {error}"))
				})?
				.flatten();
			let result = handle
				.authenticate_publickey(
					username,
					PrivateKeyWithHashAlg::new(Arc::new(private_key), hash_alg),
				)
				.await
				.map_err(|error| {
					terminal_failure(format!(
						"Private-key authentication exchange failed: {error}"
					))
				})?;
			require_success(&result, "Private-key")?;
			Ok(reusable_noninteractively)
		}
		Method::AgentKey { credential } => {
			let mut agent = AgentClient::connect_env()
				.await
				.map_err(|error| failure(format!("Failed to connect to SSH agent: {error}")))?;
			let identities = agent.request_identities().await.map_err(|error| {
				failure(format!("Failed to request SSH agent identities: {error}"))
			})?;
			let identity = identities
				.into_iter()
				.find(|identity| credential.matches_identity(identity))
				.ok_or_else(|| failure("The selected SSH agent identity is no longer available"))?;
			let hash_alg = handle
				.best_supported_rsa_hash()
				.await
				.map_err(|error| {
					terminal_failure(format!("Failed to negotiate key algorithm: {error}"))
				})?
				.flatten();
			let result = match identity {
				AgentIdentity::PublicKey { key, .. } => {
					handle
						.authenticate_publickey_with(username, key, hash_alg, &mut agent)
						.await
				}
				AgentIdentity::Certificate { certificate, .. } => {
					handle
						.authenticate_certificate_with(username, certificate, hash_alg, &mut agent)
						.await
				}
			}
			.map_err(|error| failure(format!("SSH agent signing failed: {error}")))?;
			require_success(&result, "SSH agent key")?;
			Ok(true)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{ssh_private_key_from_seed, write_test_ssh_private_key};
	use pretty_assertions::{assert_eq, assert_ne};
	use russh::keys::load_public_key;
	use russh::keys::ssh_key::{Cipher, Kdf, LineEnding};
	use russh::{MethodKind, MethodSet};
	use std::fs;

	const KEY: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T";
	const CERTIFICATE: &str = "ssh-ed25519-cert-v01@openssh.com AAAAIHNzaC1lZDI1NTE5LWNlcnQtdjAxQG9wZW5zc2guY29tAAAAIBW/4zLqXWROWmN1sPgdySnH1GUsEFBjFrRwKKw71BoBAAAAIH1MFwI1oRdEifXgBQvWQfCBBtA/Pi8YCUE/I3wXFJo2AAAAAAAAAAAAAAABAAAAA2ZvbwAAAAAAAAAAAAAAAH//////////AAAAIwAAABFoZWxsb0BleGFtcGxlLmNvbQAAAAoAAAAGZm9vYmFyAAAAAAAAAAAAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIH1MFwI1oRdEifXgBQvWQfCBBtA/Pi8YCUE/I3wXFJo2AAAAUwAAAAtzc2gtZWQyNTUxOQAAAEDRoPdI48KyoaLgaDZsSGs80qBeYQOXBd84CX8GYzFt/L21rxF1EeuPOkgsx7Q39WllXp+FgMMojsHftK/DJHEN";

	fn public_key() -> PublicKey {
		PublicKey::from_openssh(KEY).expect("static key is valid")
	}

	fn fixture_private_key(directory: &Path) -> Result<PathBuf> {
		let path = directory.join("id_ed25519");
		write_test_ssh_private_key(&path)?;
		Ok(path)
	}

	fn agent_method() -> Method {
		Method::with_agent_credential(AgentCredential::from_public_key(public_key()))
	}

	#[test]
	fn key_file_method_preserves_interaction_policy() {
		assert_eq!(
			Method::with_key_file("id_ed25519", true),
			Method::PrivateKeyFile {
				key_file_path: PathBuf::from("id_ed25519"),
				allow_prompt: true,
				fallback_on_load_failure: false,
			}
		);
		assert_eq!(
			Method::with_automatic_key_file("id_ed25519", false),
			Method::PrivateKeyFile {
				key_file_path: PathBuf::from("id_ed25519"),
				allow_prompt: false,
				fallback_on_load_failure: true,
			}
		);
	}

	#[test]
	fn password_debug_is_redacted() {
		assert_eq!(
			format!("{:?}", Method::with_password("secret")),
			"Password(\"***\")"
		);
	}

	#[test]
	fn method_descriptions_are_redacted_and_specific() {
		assert_eq!(Method::with_password("secret").description(), "password");
		assert_eq!(
			Method::with_password_prompt("Password").description(),
			"password"
		);
		assert_eq!(
			Method::with_key_file("id_ed25519", false).description(),
			"key id_ed25519"
		);
		assert!(
			agent_method()
				.description()
				.starts_with("agent key SHA256:")
		);
	}

	#[test]
	fn method_debug_output_identifies_nonsecret_candidates() {
		assert_eq!(
			format!("{:?}", Method::with_password_prompt("Password")),
			"PasswordPrompt"
		);
		assert_eq!(
			format!("{:?}", Method::with_key_file("id_ed25519", false)),
			"PrivateKeyFile { key_file_path: \"id_ed25519\", allow_prompt: false, fallback_on_load_failure: false }"
		);
		let agent_debug = format!("{:?}", agent_method());
		assert!(agent_debug.starts_with("AgentKey { credential:"));
		assert!(!agent_debug.contains("AAAAC3Nza"));
	}

	#[test]
	fn agent_credentials_distinguish_plain_keys_from_certificates() -> Result<()> {
		let certificate = Certificate::from_openssh(CERTIFICATE)?;
		let plain_identity = AgentIdentity::PublicKey {
			key: certificate.public_key().clone().into(),
			comment: "plain".to_owned(),
		};
		let certificate_identity = AgentIdentity::Certificate {
			certificate,
			comment: "certificate".to_owned(),
		};
		let plain = AgentCredential::from_identity(plain_identity.clone());
		let certified = AgentCredential::from_identity(certificate_identity.clone());

		assert_ne!(plain, certified);
		assert!(plain.matches_identity(&plain_identity));
		assert!(!plain.matches_identity(&certificate_identity));
		assert!(certified.matches_identity(&certificate_identity));
		assert!(!certified.matches_identity(&plain_identity));
		assert!(
			Method::with_agent_credential(certified)
				.description()
				.starts_with("agent certificate SHA256:")
		);
		Ok(())
	}

	#[test]
	fn authentication_failure_has_stable_marker_and_message() {
		let marker = AuthenticationFailed {
			kind: AuthenticationFailureKind::Retryable,
		};
		assert_eq!(marker.to_string(), "SSH authentication failed");
		let report = failure("candidate rejected");
		assert_eq!(
			report
				.downcast_ref::<AuthenticationFailed>()
				.map(|failure| failure.kind()),
			Some(AuthenticationFailureKind::Retryable)
		);
		assert!(report.to_string().contains("candidate rejected"));
	}

	#[test]
	fn require_success_accepts_success_and_reports_rejection() -> Result<()> {
		require_success(&AuthResult::Success, "Password")?;
		let remaining_methods = MethodSet::from(&[MethodKind::PublicKey][..]);
		let error = require_success(
			&AuthResult::Failure {
				remaining_methods,
				partial_success: false,
			},
			"Password",
		)
		.expect_err("rejected credential must fail");
		assert!(
			error
				.to_string()
				.contains("Password authentication was rejected")
		);
		assert_eq!(
			error
				.downcast_ref::<AuthenticationFailed>()
				.map(|failure| failure.kind()),
			Some(AuthenticationFailureKind::Retryable)
		);
		Ok(())
	}

	#[test]
	fn require_success_rejects_multifactor_continuation() {
		let error = require_success(
			&AuthResult::Failure {
				remaining_methods: MethodSet::from(&[MethodKind::Password][..]),
				partial_success: true,
			},
			"Public-key",
		)
		.expect_err("MFA continuation is unsupported");
		assert!(
			error
				.to_string()
				.contains("requires additional authentication")
		);
		assert_eq!(
			error
				.downcast_ref::<AuthenticationFailed>()
				.map(|failure| failure.kind()),
			Some(AuthenticationFailureKind::Terminal)
		);
	}

	#[test]
	fn load_private_key_accepts_unencrypted_key() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = fixture_private_key(dir.path())?;
		let (key, reusable_noninteractively) = load_private_key(&path, false, false)?;
		let expected = load_public_key(
			Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh/id_ed25519.pub"),
		)?;
		assert_eq!(key.public_key().key_data(), expected.key_data());
		assert!(reusable_noninteractively);
		Ok(())
	}

	#[test]
	fn load_private_key_reports_malformed_input() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("invalid-key");
		fs::write(&path, "not a private key")?;

		let error =
			load_private_key(&path, false, false).expect_err("malformed explicit key must fail");
		assert!(error.to_string().contains("Failed to load SSH private key"));
		assert_eq!(
			error
				.downcast_ref::<AuthenticationFailed>()
				.map(|failure| failure.kind()),
			Some(AuthenticationFailureKind::Terminal)
		);
		Ok(())
	}

	#[test]
	fn malformed_automatic_key_allows_candidate_fallback() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("invalid-key");
		fs::write(&path, "not a private key")?;

		let error =
			load_private_key(&path, false, true).expect_err("automatic malformed key must fail");
		assert_eq!(
			error
				.downcast_ref::<AuthenticationFailed>()
				.map(|failure| failure.kind()),
			Some(AuthenticationFailureKind::Retryable)
		);
		Ok(())
	}

	#[test]
	fn noninteractive_encrypted_key_uses_source_fallback_policy() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("encrypted-key");
		let encrypted = ssh_private_key_from_seed(&[42; 32]).encrypt_with(
			Cipher::Aes256Ctr,
			Kdf::Bcrypt {
				salt: vec![7; 16],
				rounds: 1,
			},
			42,
			"passphrase",
		)?;
		fs::write(&path, encrypted.to_openssh(LineEnding::LF)?.as_bytes())?;

		for (fallback_on_load_failure, expected) in [
			(false, AuthenticationFailureKind::Terminal),
			(true, AuthenticationFailureKind::Retryable),
		] {
			let error = load_private_key(&path, false, fallback_on_load_failure)
				.expect_err("encrypted key needs interaction");
			assert_eq!(
				error
					.downcast_ref::<AuthenticationFailed>()
					.map(|failure| failure.kind()),
				Some(expected)
			);
		}
		Ok(())
	}
}
