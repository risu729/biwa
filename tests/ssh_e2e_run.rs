#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
use std::io::{BufRead as _, BufReader, Read as _};

mod common;
use color_eyre::eyre::WrapErr as _;
use common::{Result, biwa_cmd};
use rstest::rstest;
use std::{fs, path::PathBuf};

#[test]
fn e2e_run_command() -> Result<()> {
	let output = biwa_cmd(&["run", "--skip-sync", "echo", "hello e2e from biwa"])
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);

	assert!(output.status.success());
	assert!(stdout.contains("hello e2e from biwa"));
	Ok(())
}

#[test]
fn e2e_run_stdout_stderr() -> Result<()> {
	let output = biwa_cmd(&[
		"run",
		"--skip-sync",
		"--",
		"bash",
		"-c",
		"echo 'out'; echo 'err' >&2",
	])
	.env("BIWA_LOG_QUIET", "true")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success());
	assert!(stdout.contains("out"), "stdout: {stdout}");
	assert!(stderr.contains("err"), "stderr: {stderr}");
	Ok(())
}

#[test]
fn e2e_run_streaming() -> Result<()> {
	let mut reader = biwa_cmd(&[
		"run",
		"--skip-sync",
		"--",
		"bash",
		"-c",
		"echo 'start'; sleep 0.5; echo 'end'",
	])
	.env("BIWA_LOG_QUIET", "true")
	.reader()?;

	let mut buf_reader = BufReader::new(&mut reader);

	let mut first_line = String::new();
	buf_reader.read_line(&mut first_line)?;

	// We should read 'start' immediately without waiting for 'end'
	assert!(
		first_line.contains("start"),
		"Expected 'start', got: {first_line}"
	);

	let mut rest = String::new();
	buf_reader.read_to_string(&mut rest)?;
	assert!(rest.contains("end"));
	Ok(())
}

#[test]
fn e2e_run_quiet() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "echo", "hello quiet"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success());
	assert!(stdout.contains("hello quiet"));

	// CLI prefix "$ echo hello quiet" should NOT be printed
	assert!(!stderr.contains("$ echo hello quiet"));
	assert!(!stdout.contains("$ echo hello quiet"));
	Ok(())
}

#[test]
fn e2e_run_silent() -> Result<()> {
	let output = biwa_cmd(&["--silent", "run", "--skip-sync", "echo", "hello silent"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success());
	assert!(stdout.trim().is_empty(), "stdout was not empty: {stdout}");
	assert!(stderr.trim().is_empty(), "stderr was not empty: {stderr}");
	Ok(())
}

#[test]
fn e2e_run_exit_code() -> Result<()> {
	let output = biwa_cmd(&["run", "--skip-sync", "--", "bash", "-c", "exit 42"])
		.env("BIWA_LOG_QUIET", "true")
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(!output.status.success());

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		stderr.contains("Remote command exited with code 42"),
		"stderr was: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_run_remote_dir() -> Result<()> {
	let output = biwa_cmd(&["run", "-d", "/tmp", "pwd"])
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);

	assert!(output.status.success());
	pretty_assertions::assert_eq!(stdout.trim(), "/tmp");
	Ok(())
}

#[test]
fn e2e_run_remote_dir_tilde() -> Result<()> {
	let home_output = biwa_cmd(&["run", "--skip-sync", "sh", "-c", "echo $HOME"])
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let home_dir = String::from_utf8_lossy(&home_output.stdout)
		.trim()
		.to_owned();

	let output = biwa_cmd(&["run", "-d", "~", "pwd"])
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);

	assert!(output.status.success());
	pretty_assertions::assert_eq!(stdout.trim(), home_dir);
	Ok(())
}

#[test]
fn e2e_run_env_transfer_from_flag() -> Result<()> {
	let output = biwa_cmd(&[
		"run",
		"--skip-sync",
		"--env",
		"NODE_ENV",
		"sh",
		"-c",
		"echo $NODE_ENV",
	])
	.env("BIWA_LOG_QUIET", "true")
	.env("NODE_ENV", "development")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout).trim(),
		"development"
	);
	Ok(())
}

#[test]
fn e2e_run_env_literal_from_flag() -> Result<()> {
	let output = biwa_cmd(&[
		"run",
		"--skip-sync",
		"--env",
		"NODE_ENV=production",
		"sh",
		"-c",
		"echo $NODE_ENV",
	])
	.env("BIWA_LOG_QUIET", "true")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "production");
	Ok(())
}

/// Implicit `biwa <args>` and `biwa run <args>` must use the same remote working directory.
#[test]
fn e2e_implicit_run_same_working_dir_as_explicit_run() -> Result<()> {
	// Disable auto-sync so both commands just resolve and use the same project dir without syncing.
	let explicit = biwa_cmd(&["run", "--skip-sync", "pwd"])
		.env("BIWA_LOG_QUIET", "true")
		.env("BIWA_SYNC_AUTO", "false")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let implicit = biwa_cmd(&["pwd"])
		.env("BIWA_LOG_QUIET", "true")
		.env("BIWA_SYNC_AUTO", "false")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		explicit.status.success(),
		"biwa run pwd failed: {}",
		String::from_utf8_lossy(&explicit.stderr)
	);
	assert!(
		implicit.status.success(),
		"biwa pwd failed: {}",
		String::from_utf8_lossy(&implicit.stderr)
	);

	let explicit_dir = String::from_utf8_lossy(&explicit.stdout).trim().to_owned();
	let implicit_dir = String::from_utf8_lossy(&implicit.stdout).trim().to_owned();
	pretty_assertions::assert_eq!(
		implicit_dir,
		explicit_dir,
		"implicit run and explicit run must resolve to the same remote working directory"
	);
	Ok(())
}

#[test]
fn e2e_implicit_run_command_executes_in_resolved_dir() -> Result<()> {
	// Implicit run should run in the resolved project dir, not remote home.
	let output = biwa_cmd(&["pwd"])
		.env("BIWA_LOG_QUIET", "true")
		.env("BIWA_SYNC_AUTO", "false")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		output.status.success(),
		"biwa pwd failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
	// Default remote project dir is ~/.cache/biwa/projects/<name>-<hash>
	assert!(
		stdout.contains(".cache/biwa/projects/"),
		"expected path under .cache/biwa/projects/, got: {stdout}"
	);
	Ok(())
}

/// CLI loads config from each schema fixture when used as biwa.toml.
#[rstest]
fn e2e_run_config_from_schema_fixture(
	#[files("schema/fixtures/toml/*.toml")] fixture: PathBuf,
) -> Result<()> {
	let dir = tempfile::tempdir()?;
	let target_path = dir.path().join("biwa.toml");

	fs::copy(&fixture, &target_path).wrap_err_with(|| {
		format!(
			"failed to copy {} to {}",
			fixture.display(),
			target_path.display()
		)
	})?;

	let output = biwa_cmd(&["run", "--skip-sync", ":"])
		.dir(dir.path())
		.env("NODE_ENV", "test")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		output.status.success(),
		"fixture {}: biwa run failed: {}",
		fixture.display(),
		String::from_utf8_lossy(&output.stderr)
	);
	Ok(())
}
