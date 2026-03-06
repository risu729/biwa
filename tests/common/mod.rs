#[expect(clippy::disallowed_types, reason = "This is the Result type for integration tests.")]
pub type Result<T> = color_eyre::Result<T>;

#[cfg(test)]
#[ctor::ctor]
fn init_test_env() {
	#[expect(
		clippy::unused_result_ok,
		reason = "Multiple tests may attempt to initialize the global error handler."
	)]
	color_eyre::install().ok();
}
