/// Shared test utilities — only compiled in `cfg(test)` contexts.
use std::env;

/// RAII guard that removes an environment variable when dropped.
///
/// Ensures clean-up even if the test panics.
pub struct EnvCleanup(pub &'static str);

impl Drop for EnvCleanup {
	fn drop(&mut self) {
		// SAFETY: Tests using this guard must be annotated with `#[serial]`
		// (from the `serial_test` crate) to prevent concurrent env mutation.
		unsafe {
			env::remove_var(self.0);
		}
	}
}
