#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]

use std::fs;

mod common;
use common::{Result, biwa_cmd};

fn run_with_invalid_config(extra_env: &[(&str, &str)]) -> Result<String> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("biwa.toml"), "invalid = = toml")?;

	let mut cmd = biwa_cmd(&["run", "--skip-sync", "echo", "hello"])
		.dir(dir.path())
		.env("NO_COLOR", "1")
		.stderr_capture()
		.unchecked();

	for (name, value) in extra_env {
		cmd = cmd.env(*name, *value);
	}

	let output = cmd.run()?;
	assert!(!output.status.success(), "command should fail");
	Ok(String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn default_error_report_hides_internal_traces() -> Result<()> {
	let stderr = run_with_invalid_config(&[("RUST_LIB_BACKTRACE", "1")])?;

	assert!(
		stderr.contains("Failed to parse TOML"),
		"stderr was: {stderr}"
	);
	assert!(!stderr.contains("BACKTRACE"), "stderr was: {stderr}");
	assert!(!stderr.contains("SPANTRACE"), "stderr was: {stderr}");
	assert!(!stderr.contains(".cargo/registry"), "stderr was: {stderr}");
	assert!(!stderr.contains("/rustc/"), "stderr was: {stderr}");
	Ok(())
}

#[test]
fn debug_error_report_shows_detailed_traces() -> Result<()> {
	let stderr = run_with_invalid_config(&[
		("BIWA_DEBUG_ERROR_REPORT", "1"),
		("RUST_LIB_BACKTRACE", "1"),
	])?;

	assert!(
		stderr.contains("Failed to parse TOML"),
		"stderr was: {stderr}"
	);
	assert!(
		stderr.contains("BACKTRACE")
			|| stderr.contains("SPANTRACE")
			|| stderr.contains(".cargo/registry")
			|| stderr.contains("/rustc/"),
		"stderr was: {stderr}"
	);
	Ok(())
}
