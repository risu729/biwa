/// Shared test utilities — only compiled in `cfg(test)` contexts.
use std::env;
use std::ffi::OsString;

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
