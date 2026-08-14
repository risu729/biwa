/// Shared test utilities — only compiled in `cfg(test)` contexts.
use std::env;
use std::ffi::OsString;
use std::path::Path;

use crate::Result;
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};

/// Fixed seed for the disposable SSH key authorized by the E2E test server.
pub const TEST_SSH_PRIVATE_KEY_SEED: [u8; 32] = [
	0x5a, 0x1d, 0x94, 0x0a, 0x2e, 0xff, 0x11, 0xac, 0x5b, 0xd8, 0xf8, 0xa0, 0x66, 0x3f, 0x53, 0x7b,
	0x4c, 0xb3, 0x45, 0xcf, 0xce, 0x5e, 0x8f, 0x13, 0xe0, 0xa4, 0x59, 0xa7, 0xae, 0x45, 0xd7, 0x15,
];

/// Derives a deterministic disposable SSH private key for tests.
#[must_use]
pub fn ssh_private_key_from_seed(seed: &[u8; 32]) -> PrivateKey {
	PrivateKey::from(Ed25519Keypair::from_seed(seed))
}

/// Writes the E2E test private key without keeping PEM material in the repository.
pub fn write_test_ssh_private_key(path: &Path) -> Result<PublicKey> {
	let mut key = ssh_private_key_from_seed(&TEST_SSH_PRIVATE_KEY_SEED);
	key.set_comment("biwa-e2e");
	key.write_openssh_file(path, LineEnding::LF)?;
	Ok(key.public_key().clone())
}

/// RAII guard that restores the previous environment variable state when dropped.
///
/// Ensures clean-up even if the test panics.
pub struct EnvCleanup {
	name: &'static str,
	previous: Option<OsString>,
}

impl EnvCleanup {
	/// Sets an environment variable for the duration of a test.
	#[must_use]
	pub fn set(name: &'static str, value: &str) -> Self {
		let previous = env::var_os(name);
		// SAFETY: Tests using this helper must be annotated with `#[serial]`
		// (from the `serial_test` crate) to prevent concurrent env mutation.
		unsafe {
			env::set_var(name, value);
		}
		Self { name, previous }
	}

	/// Removes an environment variable for the duration of a test.
	#[must_use]
	pub fn remove(name: &'static str) -> Self {
		let previous = env::var_os(name);
		// SAFETY: Tests using this helper must be annotated with `#[serial]`
		// (from the `serial_test` crate) to prevent concurrent env mutation.
		unsafe {
			env::remove_var(name);
		}
		Self { name, previous }
	}
}

impl Drop for EnvCleanup {
	fn drop(&mut self) {
		if let Some(previous) = &self.previous {
			// SAFETY: Tests using this guard must be annotated with `#[serial]`
			// (from the `serial_test` crate) to prevent concurrent env mutation.
			unsafe {
				env::set_var(self.name, previous);
			}
		} else {
			// SAFETY: Tests using this guard must be annotated with `#[serial]`
			// (from the `serial_test` crate) to prevent concurrent env mutation.
			unsafe {
				env::remove_var(self.name);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use serial_test::serial;

	#[test]
	#[serial]
	fn env_cleanup_restores_previous_value() {
		// SAFETY: This test is `#[serial]` and restores env via `EnvCleanup`.
		unsafe {
			env::set_var("BIWA_TEST_ENV_CLEANUP", "before");
		}

		{
			let _cleanup = EnvCleanup::set("BIWA_TEST_ENV_CLEANUP", "after");
			assert_eq!(env::var("BIWA_TEST_ENV_CLEANUP").as_deref(), Ok("after"));
		}

		assert_eq!(env::var("BIWA_TEST_ENV_CLEANUP").as_deref(), Ok("before"));

		// SAFETY: This test is `#[serial]`.
		unsafe {
			env::remove_var("BIWA_TEST_ENV_CLEANUP");
		}
	}

	#[test]
	#[serial]
	fn env_cleanup_restores_missing_value_after_remove() {
		// SAFETY: This test is `#[serial]`.
		unsafe {
			env::remove_var("BIWA_TEST_ENV_CLEANUP");
		}

		{
			let _cleanup = EnvCleanup::remove("BIWA_TEST_ENV_CLEANUP");
			assert_eq!(
				env::var("BIWA_TEST_ENV_CLEANUP"),
				Err(env::VarError::NotPresent)
			);
		}

		assert_eq!(
			env::var("BIWA_TEST_ENV_CLEANUP"),
			Err(env::VarError::NotPresent)
		);
	}
}
