#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
use std::io::{BufRead as _, BufReader, Read as _};

mod common;
use common::biwa_cmd;

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_command() {
	let output = biwa_cmd(&["run", "--no-sync", "echo", "hello e2e from biwa"])
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.expect("failed to execute process");

	let stdout = String::from_utf8_lossy(&output.stdout);

	assert!(output.status.success());
	assert!(stdout.contains("hello e2e from biwa"));
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_stdout_stderr() {
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
	.run()
	.expect("failed to execute process");

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success());
	assert!(stdout.contains("out"), "stdout: {stdout}");
	assert!(stderr.contains("err"), "stderr: {stderr}");
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_streaming() {
	let mut reader = biwa_cmd(&[
		"run",
		"--no-sync",
		"--",
		"bash",
		"-c",
		"echo 'start'; sleep 0.5; echo 'end'",
	])
	.env("BIWA_LOG_QUIET", "true")
	.reader()
	.expect("failed to spawn process");

	let mut buf_reader = BufReader::new(&mut reader);

	let mut first_line = String::new();
	buf_reader
		.read_line(&mut first_line)
		.expect("failed to read first line");

	// We should read 'start' immediately without waiting for 'end'
	assert!(
		first_line.contains("start"),
		"Expected 'start', got: {first_line}"
	);

	let mut rest = String::new();
	buf_reader
		.read_to_string(&mut rest)
		.expect("failed to read remaining output");
	assert!(rest.contains("end"));
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_quiet() {
	let output = biwa_cmd(&["--quiet", "run", "--no-sync", "echo", "hello quiet"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.expect("failed to execute process");

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success());
	assert!(stdout.contains("hello quiet"));

	// CLI prefix "$ echo hello quiet" should NOT be printed
	assert!(!stderr.contains("$ echo hello quiet"));
	assert!(!stdout.contains("$ echo hello quiet"));
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_silent() {
	let output = biwa_cmd(&["--silent", "run", "--no-sync", "echo", "hello silent"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.expect("failed to execute process");

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success());
	assert!(stdout.trim().is_empty(), "stdout was not empty: {stdout}");
	assert!(stderr.trim().is_empty(), "stderr was not empty: {stderr}");
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_run_exit_code() {
	let output = biwa_cmd(&["run", "--no-sync", "--", "bash", "-c", "exit 42"])
		.env("BIWA_LOG_QUIET", "true")
		.stderr_capture()
		.unchecked()
		.run()
		.expect("failed to execute process");

	assert!(!output.status.success());

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		stderr.contains("Remote command exited with code 42"),
		"stderr was: {stderr}"
	);
}
