#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
use std::io::{BufRead as _, BufReader, Read as _};

use core::time::Duration;
mod common;
use color_eyre::eyre::{WrapErr as _, eyre};
use common::{Result, biwa_cmd};
use rstest::rstest;
use std::{
	env, ffi::OsStr, fs, path::PathBuf, process::Command, process::Stdio, thread, time::Instant,
};

fn e2e_timeout_secs() -> u64 {
	env::var("BIWA_E2E_TIMEOUT_SECS")
		.ok()
		.and_then(|value| value.parse::<u64>().ok())
		.filter(|value| *value > 0)
		.unwrap_or(10)
}

fn biwa_process(args: &[&str]) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_biwa"));
	command
		.args(args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123");
	command
}

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
		"--quiet",
		"run",
		"--skip-sync",
		"--",
		"bash",
		"-c",
		"echo 'out'; echo 'err' >&2",
	])
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
		"--quiet",
		"run",
		"--skip-sync",
		"--",
		"bash",
		"-c",
		"echo 'start'; sleep 0.5; echo 'end'",
	])
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
fn e2e_run_with_tty_stdin_exits_without_waiting_for_input() -> Result<()> {
	let timeout_secs = e2e_timeout_secs();
	let python = format!(
		r#"import os, pty, subprocess, sys, time
master, slave = pty.openpty()
try:
    proc = subprocess.Popen(
        [{biwa_path:?}, "--quiet", "run", "--skip-sync", "pwd"],
        stdin=slave,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=os.environ.copy(),
    )
finally:
    os.close(slave)

deadline = time.time() + {timeout_secs}
while proc.poll() is None and time.time() < deadline:
    time.sleep(0.05)

if proc.poll() is None:
    proc.kill()
    out, err = proc.communicate()
    sys.stderr.write("timed out while waiting for biwa to exit\n")
    sys.stderr.buffer.write(out)
    sys.stderr.buffer.write(err)
    sys.exit(124)

os.close(master)
out, err = proc.communicate()
sys.stdout.buffer.write(out)
sys.stderr.buffer.write(err)
sys.exit(proc.returncode)
"#,
		biwa_path = env!("CARGO_BIN_EXE_biwa"),
		timeout_secs = timeout_secs,
	);

	let output = Command::new("python3")
		.arg("-c")
		.arg(&python)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123")
		.output()?;

	assert!(
		output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(
		String::from_utf8_lossy(&output.stdout)
			.trim()
			.contains(".cache/biwa/projects/"),
		"stdout: {}",
		String::from_utf8_lossy(&output.stdout)
	);
	Ok(())
}

#[test]
fn e2e_run_forwards_tty_stdin() -> Result<()> {
	let timeout_secs = e2e_timeout_secs();
	let python = format!(
		r#"import os, pty, select, subprocess, sys
master, slave = pty.openpty()
try:
    proc = subprocess.Popen(
        [{biwa_path:?}, "--quiet", "run", "--skip-sync", "cat"],
        stdin=slave,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=os.environ.copy(),
    )
finally:
    os.close(slave)

os.write(master, b"hello from tty stdin\n")
ready, _, _ = select.select([proc.stdout], [], [], {timeout_secs})
if not ready:
    proc.kill()
    out, err = proc.communicate()
    sys.stderr.write("timed out while waiting for biwa to echo tty stdin\n")
    sys.stderr.buffer.write(out)
    sys.stderr.buffer.write(err)
    sys.exit(124)

line = proc.stdout.readline()
sys.stdout.buffer.write(line)
proc.kill()
_, err = proc.communicate()
sys.stderr.buffer.write(err)
if line.replace(b"\r\n", b"\n") != b"hello from tty stdin\n":
    sys.stderr.write(f"unexpected stdout line: {{line!r}}\n")
    sys.exit(1)
sys.exit(0)
"#,
		biwa_path = env!("CARGO_BIN_EXE_biwa"),
		timeout_secs = timeout_secs,
	);

	let output = Command::new("python3")
		.arg("-c")
		.arg(&python)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123")
		.output()?;

	assert!(
		output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
		"hello from tty stdin\n"
	);
	Ok(())
}

#[test]
fn e2e_run_forwards_stdin() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "cat"])
		.stdin_bytes("hello from stdin\n")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout),
		"hello from stdin\n"
	);
	Ok(())
}

#[test]
fn e2e_run_forwards_stdin_with_setenv_forward_method() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(
		dir.path().join("biwa.toml"),
		"[env]\nforward_method = \"setenv\"\n",
	)?;

	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "cat"])
		.dir(dir.path())
		.stdin_bytes("hello from stdin via setenv\n")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout),
		"hello from stdin via setenv\n"
	);
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
fn e2e_run_silent_large_output() -> Result<()> {
	const OUTPUT_LINES: usize = 4096;
	let command = format!(
		"for i in $(seq 1 {OUTPUT_LINES}); do printf 'out%04d\\n' \"$i\"; done & \
		 for i in $(seq 1 {OUTPUT_LINES}); do printf 'err%04d\\n' \"$i\" >&2; done & \
		 wait"
	);

	let mut child = biwa_process(&[
		"--silent",
		"run",
		"--skip-sync",
		"--",
		"bash",
		"-c",
		&command,
	]);
	child.stdout(Stdio::piped()).stderr(Stdio::piped());
	let mut child = child.spawn()?;

	let deadline = Instant::now() + Duration::from_secs(20);
	while child.try_wait()?.is_none() {
		if Instant::now() >= deadline {
			#[expect(
				clippy::unused_result_ok,
				reason = "The process may already have exited between try_wait and kill."
			)]
			child.kill().ok();
			let output = child.wait_with_output()?;
			let stdout = String::from_utf8_lossy(&output.stdout);
			let stderr = String::from_utf8_lossy(&output.stderr);
			return Err(eyre!(
				"silent large-output run timed out, likely deadlocked\nstdout: {stdout}\nstderr: {stderr}"
			));
		}
		thread::sleep(Duration::from_millis(50));
	}

	let output = child.wait_with_output()?;
	let success = output.status.success();
	let stdout = output.stdout;
	let stderr = output.stderr;

	let stdout = String::from_utf8_lossy(&stdout);
	let stderr = String::from_utf8_lossy(&stderr);

	assert!(success, "stderr: {stderr}");
	assert!(stdout.trim().is_empty(), "stdout was not empty: {stdout}");
	assert!(stderr.trim().is_empty(), "stderr was not empty: {stderr}");
	Ok(())
}

#[test]
fn e2e_run_exit_code() -> Result<()> {
	let output = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--",
		"bash",
		"-c",
		"exit 42",
	])
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
	let output = biwa_cmd(&["--quiet", "run", "-d", "/tmp", "pwd"])
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
	let home_output = biwa_cmd(&["--quiet", "run", "--skip-sync", "sh", "-c", "echo $HOME"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let home_dir = String::from_utf8_lossy(&home_output.stdout)
		.trim()
		.to_owned();

	let output = biwa_cmd(&["--quiet", "run", "-d", "~", "pwd"])
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
fn e2e_run_env_forward_from_flag() -> Result<()> {
	let output = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"NODE_ENV",
		"sh",
		"-c",
		"echo $NODE_ENV",
	])
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
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"NODE_ENV=production",
		"sh",
		"-c",
		"echo $NODE_ENV",
	])
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "production");
	Ok(())
}

#[test]
fn e2e_run_env_literal_empty_from_flag() -> Result<()> {
	let output = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"NODE_ENV=",
		"sh",
		"-c",
		"if [ \"${NODE_ENV+x}\" = x ]; then printf 'set:%s' \"$NODE_ENV\"; else printf missing; fi",
	])
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "set:");
	Ok(())
}

#[test]
fn e2e_run_env_forward_empty_from_flag() -> Result<()> {
	let output = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"NODE_ENV",
		"sh",
		"-c",
		"if [ \"${NODE_ENV+x}\" = x ]; then printf 'set:%s' \"$NODE_ENV\"; else printf missing; fi",
	])
	.env("NODE_ENV", "")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "set:");
	Ok(())
}

#[test]
fn e2e_run_env_wildcard_and_negation_from_flag() -> Result<()> {
	let output = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"NODE_*",
		"--env",
		"!*PATH",
		"sh",
		"-c",
		"printf '%s|' \"$NODE_ENV\"; if [ -n \"$NODE_PATH\" ]; then printf present; else printf missing; fi",
	])
	.env("NODE_ENV", "development")
	.env("NODE_PATH", "/tmp/node")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	assert!(output.status.success());
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout).trim(),
		"development|missing"
	);
	Ok(())
}

/// Implicit `biwa <args>` and `biwa run <args>` must use the same remote working directory.
#[test]
fn e2e_implicit_run_same_working_dir_as_explicit_run() -> Result<()> {
	// Disable auto-sync so both commands just resolve and use the same project dir without syncing.
	let explicit = biwa_cmd(&["--quiet", "run", "--skip-sync", "pwd"])
		.env("BIWA_SYNC_AUTO", "false")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let implicit = biwa_cmd(&["--quiet", "pwd"])
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
	let output = biwa_cmd(&["--quiet", "pwd"])
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

	let fixture_name = fixture
		.file_name()
		.and_then(OsStr::to_str)
		.unwrap_or_default();

	if fixture_name == "edge-env-forward-setenv.toml" {
		if output.status.success() {
			return Ok(());
		}
		assert!(
			String::from_utf8_lossy(&output.stderr)
				.contains("rejected environment variable forwarding via setenv"),
			"fixture {}: expected either setenv success or a setenv rejection, got: {}",
			fixture.display(),
			String::from_utf8_lossy(&output.stderr)
		);
		return Ok(());
	}

	assert!(
		output.status.success(),
		"fixture {}: biwa run failed: {}",
		fixture.display(),
		String::from_utf8_lossy(&output.stderr)
	);
	Ok(())
}
