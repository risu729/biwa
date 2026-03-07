#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
use std::io::{BufRead as _, BufReader, Read as _};

mod common;
use common::{Result, biwa_cmd};

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_command() -> Result<()> {
	let output = biwa_cmd(&["run", "--no-sync", "echo", "hello e2e from biwa"])
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
#[ignore = "requires running SSH server"]
fn e2e_run_stdout_stderr() -> Result<()> {
	let output = biwa_cmd(&[
		"run",
		"--no-sync",
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
#[ignore = "requires running SSH server"]
fn e2e_run_streaming() -> Result<()> {
	let mut reader = biwa_cmd(&[
		"run",
		"--no-sync",
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
#[ignore = "requires running SSH server"]
fn e2e_run_quiet() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--no-sync", "echo", "hello quiet"])
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
#[ignore = "requires running SSH server"]
fn e2e_run_silent() -> Result<()> {
	let output = biwa_cmd(&["--silent", "run", "--no-sync", "echo", "hello silent"])
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
#[ignore = "requires running SSH server"]
fn e2e_run_exit_code() -> Result<()> {
	let output = biwa_cmd(&["run", "--no-sync", "--", "bash", "-c", "exit 42"])
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
#[ignore = "requires running SSH server"]
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
#[ignore = "requires running SSH server"]
fn e2e_run_remote_dir_tilde() -> Result<()> {
	let home_output = biwa_cmd(&["run", "--no-sync", "sh", "-c", "echo $HOME"])
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
