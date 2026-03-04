/// Shared test utilities — only compiled in `cfg(test)` contexts.
use std::env;
use std::sync::Mutex;

/// A global mutex to serialize tests that mutate environment variables.
///
/// Tests that call [`env::set_var`] or [`env::remove_var`] must hold this
/// before making any changes.
pub static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// RAII guard that removes an environment variable when dropped.
///
/// Ensures clean-up even if the test panics.
pub struct EnvCleanup(pub &'static str);

impl Drop for EnvCleanup {
	fn drop(&mut self) {
		// SAFETY: The caller must hold `ENV_MUTEX` for the lifetime of this guard,
		// ensuring no concurrent env mutation from other tests.
		unsafe {
			env::remove_var(self.0);
		}
	}
}
