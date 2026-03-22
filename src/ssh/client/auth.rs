use crate::Result;
use alloc::sync::Arc;
use color_eyre::eyre::{Context as _, Report};
use core::error::Error as StdError;
use core::fmt;
use russh::client::{Handle, Handler};
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key};
use std::path::{Path, PathBuf};

/// Marker error for SSH authentication failures.
///
/// Returned from [`authenticate`] so callers can detect auth failures without matching error strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationFailed;

impl fmt::Display for AuthenticationFailed {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("SSH authentication failed")
	}
}

impl StdError for AuthenticationFailed {}

/// An authentication token.
///
/// Used when creating a `Client` for authentication.
#[derive(Clone, PartialEq, Eq)]
pub enum Method {
	/// Password authentication.
	Password(String),
	/// Private key file authentication.
	PrivateKeyFile {
		/// Path to the private key file.
		key_file_path: PathBuf,
		/// Optional passphrase for the private key.
		key_pass: Option<String>,
	},
	/// SSH Agent authentication.
	Agent,
}

impl Method {
	/// Creates a password authentication method.
	pub fn with_password<S: Into<String>>(password: S) -> Self {
		Self::Password(password.into())
	}

	/// Creates a private key file authentication method.
	pub fn with_key_file<T: AsRef<Path>>(key_file_path: T, passphrase: Option<&str>) -> Self {
		Self::PrivateKeyFile {
			key_file_path: key_file_path.as_ref().to_path_buf(),
			key_pass: passphrase.map(str::to_string),
		}
	}

	/// Creates an SSH agent authentication method.
	pub const fn with_agent() -> Self {
		Self::Agent
	}
}

impl fmt::Debug for Method {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Password(_) => f.debug_tuple("Password").field(&"***").finish(),
			Self::PrivateKeyFile {
				key_file_path,
				key_pass,
			} => f
				.debug_struct("PrivateKeyFile")
				.field("key_file_path", key_file_path)
				.field("key_pass", &key_pass.as_ref().map(|_| "***"))
				.finish(),
			Self::Agent => write!(f, "Agent"),
		}
	}
}

/// Authenticate a connected handle using the given method.
pub(super) async fn authenticate<H: Handler>(
	handle: &mut Handle<H>,
	username: &str,
	auth: Method,
) -> Result<()> {
	match auth {
		Method::Password(password) => {
			let is_authenticated = handle.authenticate_password(username, password).await?;
			if !is_authenticated.success() {
				return Err(Report::from(AuthenticationFailed))
					.wrap_err("Password authentication failed");
			}
		}
		Method::PrivateKeyFile {
			key_file_path,
			key_pass,
		} => {
			let cprivk = load_secret_key(key_file_path, key_pass.as_deref())
				.wrap_err("Failed to load secret key")?;

			let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
			let is_authenticated = handle
				.authenticate_publickey(
					username,
					PrivateKeyWithHashAlg::new(Arc::new(cprivk), hash_alg),
				)
				.await?;

			if !is_authenticated.success() {
				return Err(Report::from(AuthenticationFailed))
					.wrap_err("Key authentication failed");
			}
		}
		Method::Agent => {
			let mut agent = AgentClient::connect_env()
				.await
				.wrap_err("Failed to connect to SSH agent")?;
			let identities = agent
				.request_identities()
				.await
				.wrap_err("Failed to request identities from agent")?;

			if identities.is_empty() {
				return Err(Report::from(AuthenticationFailed))
					.wrap_err("SSH agent has no identities");
			}

			let mut authenticated = false;
			let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
			for identity in identities {
				let is_auth = handle
					.authenticate_publickey_with(username, identity, hash_alg, &mut agent)
					.await;
				if is_auth.is_ok_and(|res| res.success()) {
					authenticated = true;
					break;
				}
			}

			if !authenticated {
				return Err(Report::from(AuthenticationFailed))
					.wrap_err("Agent authentication failed");
			}
		}
	}
	Ok(())
}
