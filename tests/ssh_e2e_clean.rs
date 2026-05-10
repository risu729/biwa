#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
//! End-to-end tests for `biwa clean`.
//!
//! Cases use [`serial_test::serial`] so parallel runs do not contend on the cleanup daemon PID
//! file or `clean --auto`.

use common::Result;
use serial_test::serial;
use std::{fs, path::Path};

mod common;

fn biwa_cmd(args: &[&str], current_dir: &Path) -> duct::Expression {
	common::biwa_cmd(args).dir(current_dir)
}

fn biwa_cmd_tilde(args: &[&str], current_dir: &Path) -> duct::Expression {
	biwa_cmd(args, current_dir).env("BIWA_SYNC_REMOTE_ROOT", "~/.cache/biwa/projects")
}

#[test]
#[serial]
fn e2e_clean_dry_run_current_project() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "clean e2e")?;

	let sync_out = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let sync_stderr = String::from_utf8_lossy(&sync_out.stderr);
	assert!(
		sync_out.status.success(),
		"sync failed: stderr: {sync_stderr}"
	);

	let remote_dir = common::get_remote_project_dir(dir.path())?;
	let out = biwa_cmd_tilde(&["clean", "--dry-run"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&out.stderr);
	assert!(
		out.status.success(),
		"clean --dry-run failed: stderr: {stderr}"
	);
	assert!(
		stderr.contains("Would remove"),
		"expected dry-run message, stderr: {stderr}"
	);
	assert!(
		stderr.contains(remote_dir.trim_start_matches('~')),
		"expected remote path in stderr: {stderr}"
	);
	Ok(())
}

#[test]
#[serial]
fn e2e_clean_dry_run_all() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("tracked.txt"), "x")?;

	let sync_out = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		sync_out.status.success(),
		"sync failed: {}",
		String::from_utf8_lossy(&sync_out.stderr)
	);

	let remote_dir = common::get_remote_project_dir(dir.path())?;
	let out = biwa_cmd_tilde(&["clean", "--dry-run", "--all"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&out.stderr);
	assert!(
		out.status.success(),
		"clean --dry-run --all failed: stderr: {stderr}"
	);
	assert!(
		stderr.contains("Would remove") || stderr.contains("No tracked remote directories"),
		"unexpected stderr: {stderr}"
	);
	if stderr.contains("Would remove") {
		assert!(
			stderr.contains(remote_dir.trim_start_matches('~')),
			"expected tracked path in stderr: {stderr}"
		);
	}
	Ok(())
}

#[test]
#[serial]
fn e2e_clean_dry_run_purge() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("purge.txt"), "y")?;

	let out = biwa_cmd_tilde(&["clean", "--dry-run", "--purge"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&out.stderr);
	assert!(
		out.status.success(),
		"clean --dry-run --purge failed: stderr: {stderr}"
	);
	assert!(
		stderr.contains("Would remove") || stderr.contains("No directories found"),
		"unexpected stderr: {stderr}"
	);
	Ok(())
}

#[test]
#[serial]
fn e2e_clean_auto_exits_successfully() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("auto.txt"), "z")?;

	let sync_out = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let sync_stderr = String::from_utf8_lossy(&sync_out.stderr);
	assert!(
		sync_out.status.success(),
		"sync failed: stderr: {sync_stderr}"
	);

	let out = biwa_cmd_tilde(&["clean", "--auto", "--quiet"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&out.stderr);
	let stdout = String::from_utf8_lossy(&out.stdout);
	assert!(
		out.status.success(),
		"clean --auto --quiet failed: stdout={stdout} stderr={stderr}"
	);
	Ok(())
}

#[test]
#[serial]
fn e2e_clean_stop_when_no_daemon() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let out = biwa_cmd_tilde(&["clean", "stop"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&out.stderr);
	assert!(out.status.success(), "clean stop failed: stderr: {stderr}");
	assert!(
		stderr.contains("No background cleanup daemon") || stderr.contains("Stopped background"),
		"unexpected stderr: {stderr}"
	);
	Ok(())
}
