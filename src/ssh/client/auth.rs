use crate::Result;
use alloc::sync::Arc;
use color_eyre::eyre::Report;
use core::error::Error as StdError;
use core::fmt;
use dialoguer::Password;
use russh::client::{AuthResult, Handle, Handler};
use russh::keys::agent::{AgentIdentity, client::AgentClient};
use russh::keys::{
	Error as KeyError, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, load_secret_key,
};
use std::path::{Path, PathBuf};

/// Marker error for SSH authentication failures.
///
/// Returned from [`authenticate`] so callers can distinguish credential failures from transport
/// and host-key failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationFailed;

impl fmt::Display for AuthenticationFailed {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("SSH authentication failed")
	}
}

impl StdError for AuthenticationFailed {}

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
	},
	/// One specific identity exposed by the environment-selected SSH agent.
	AgentKey {
		/// Public key material used to find the identity again before signing.
		public_key: PublicKey,
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
		}
	}

	/// Creates a method for one concrete SSH-agent identity.
	pub const fn with_agent_key(public_key: PublicKey) -> Self {
		Self::AgentKey { public_key }
	}

	/// Returns a redacted description suitable for aggregated errors.
	#[must_use]
	pub fn description(&self) -> String {
		match self {
			Self::Password(_) | Self::PasswordPrompt { .. } => "password".to_owned(),
			Self::PrivateKeyFile { key_file_path, .. } => {
				format!("key {}", key_file_path.display())
			}
			Self::AgentKey { public_key } => {
				format!("agent key {}", public_key.fingerprint(HashAlg::Sha256))
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
			} => f
				.debug_struct("PrivateKeyFile")
				.field("key_file_path", key_file_path)
				.field("allow_prompt", allow_prompt)
				.finish(),
			Self::AgentKey { public_key } => f
				.debug_struct("AgentKey")
				.field("fingerprint", &public_key.fingerprint(HashAlg::Sha256))
				.finish(),
		}
	}
}

/// Creates a structured authentication failure with redacted context.
fn failure(context: impl fmt::Display + Send + Sync + 'static) -> Report {
	Report::from(AuthenticationFailed).wrap_err(context)
}

/// Converts a Russh authentication result into the supported single-factor contract.
fn require_success(result: &AuthResult, method: &str) -> Result<()> {
	match result {
		AuthResult::Success => Ok(()),
		AuthResult::Failure {
			partial_success: true,
			..
		} => Err(failure(
			"The server accepted one factor but requires additional authentication; MFA chains are not supported",
		)),
		AuthResult::Failure { .. } => Err(failure(format!("{method} authentication was rejected"))),
	}
}

/// Loads a private key, prompting only when this concrete candidate is reached and interaction is
/// allowed.
fn load_private_key(path: &Path, allow_prompt: bool) -> Result<PrivateKey> {
	match load_secret_key(path, None) {
		Ok(key) => Ok(key),
		Err(KeyError::KeyIsEncrypted) if allow_prompt => {
			let passphrase = Password::new()
				.with_prompt(format!("Passphrase for {}", path.display()))
				.interact()
				.map_err(|error| {
					failure(format!("Failed to read private-key passphrase: {error}"))
				})?;
			load_secret_key(path, Some(&passphrase)).map_err(|error| {
				failure(format!(
					"Failed to decrypt SSH private key {}: {error}",
					path.display()
				))
			})
		}
		Err(KeyError::KeyIsEncrypted) => Err(failure(format!(
			"SSH private key {} is encrypted and interactive input is unavailable",
			path.display()
		))),
		Err(error) => Err(failure(format!(
			"Failed to load SSH private key {}: {error}",
			path.display()
		))),
	}
}

/// Authenticates a connected handle using exactly one concrete credential.
pub(super) async fn authenticate<H: Handler>(
	handle: &mut Handle<H>,
	username: &str,
	auth: Method,
) -> Result<()> {
	match auth {
		Method::Password(password) => {
			let result = handle
				.authenticate_password(username, password)
				.await
				.map_err(|error| failure(format!("Password authentication failed: {error}")))?;
			require_success(&result, "Password")
		}
		Method::PasswordPrompt { prompt } => {
			let password = Password::new()
				.with_prompt(prompt)
				.interact()
				.map_err(|error| failure(format!("Failed to read password: {error}")))?;
			let result = handle
				.authenticate_password(username, password)
				.await
				.map_err(|error| failure(format!("Password authentication failed: {error}")))?;
			require_success(&result, "Password")
		}
		Method::PrivateKeyFile {
			key_file_path,
			allow_prompt,
		} => {
			let private_key = load_private_key(&key_file_path, allow_prompt)?;
			let hash_alg = handle
				.best_supported_rsa_hash()
				.await
				.map_err(|error| failure(format!("Failed to negotiate key algorithm: {error}")))?
				.flatten();
			let result = handle
				.authenticate_publickey(
					username,
					PrivateKeyWithHashAlg::new(Arc::new(private_key), hash_alg),
				)
				.await
				.map_err(|error| failure(format!("Private-key authentication failed: {error}")))?;
			require_success(&result, "Private-key")
		}
		Method::AgentKey { public_key } => {
			let mut agent = AgentClient::connect_env()
				.await
				.map_err(|error| failure(format!("Failed to connect to SSH agent: {error}")))?;
			let identities = agent.request_identities().await.map_err(|error| {
				failure(format!("Failed to request SSH agent identities: {error}"))
			})?;
			let identity = identities
				.into_iter()
				.find(|identity| identity.public_key().key_data() == public_key.key_data())
				.ok_or_else(|| failure("The selected SSH agent identity is no longer available"))?;
			let hash_alg = handle
				.best_supported_rsa_hash()
				.await
				.map_err(|error| failure(format!("Failed to negotiate key algorithm: {error}")))?
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
			require_success(&result, "SSH agent key")
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn key_file_method_preserves_interaction_policy() {
		assert_eq!(
			Method::with_key_file("id_ed25519", true),
			Method::PrivateKeyFile {
				key_file_path: PathBuf::from("id_ed25519"),
				allow_prompt: true,
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
}
