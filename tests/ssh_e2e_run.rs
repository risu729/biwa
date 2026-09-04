#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
use std::io::{BufRead as _, BufReader, Read as _};

use core::time::Duration;
mod common;
use color_eyre::eyre::{WrapErr as _, eyre};
use common::{
	Result, biwa_cmd, biwa_cmd_capable, biwa_program_cmd, ssh_port, test_known_hosts_path,
	write_ssh_private_key_from_seed, write_test_ssh_private_key,
};
use rstest::rstest;
use std::{
	env,
	ffi::OsStr,
	fs,
	path::{Path, PathBuf},
	process::{Child, Command, Stdio},
	thread,
	time::Instant,
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
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path());
	command
}

fn biwa_host_key_cmd(args: &[&str], checking: &str, known_hosts: &Path) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", checking)
		.env("BIWA_SSH_KNOWN_HOSTS", known_hosts)
		.env("BIWA_CLEAN_AUTO", "false")
}

fn biwa_public_key_cmd(args: &[&str], key_path: &Path) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "public-key")
		.env("BIWA_SSH_KEY_PATH", key_path)
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
		.env("BIWA_CLEAN_AUTO", "false")
}

fn biwa_agent_cmd(args: &[&str], auth_sock: &Path) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "public-key")
		.env("SSH_AUTH_SOCK", auth_sock)
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
		.env("BIWA_CLEAN_AUTO", "false")
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
	fn drop(&mut self) {
		match self.0.kill() {
			Ok(()) | Err(_) => {}
		}
		match self.0.wait() {
			Ok(_) | Err(_) => {}
		}
	}
}

const SIGINT_REMOTE_SCRIPT: &str =
	"trap 'printf interrupted; exit 130' INT; printf 'ready\\n'; while true; do sleep 1; done";

fn run_sigint_forwarding_case(use_pty: bool) -> Result<()> {
	let timeout_secs = e2e_timeout_secs();
	let use_pty = if use_pty { "True" } else { "False" };
	let python = format!(
		r#"import os, pty, select, signal, subprocess, sys
use_pty = {use_pty}
master = None
slave = None
stdin = subprocess.DEVNULL

if use_pty:
    master, slave = pty.openpty()
    stdin = slave

try:
    proc = subprocess.Popen(
        [{biwa_path:?}, "--quiet", "run", "--skip-sync", "--", "bash", "-c", {remote_script:?}],
        stdin=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=os.environ.copy(),
    )
finally:
    if slave is not None:
        os.close(slave)

def close_master():
    global master
    if master is not None:
        os.close(master)
        master = None

def kill_and_fail(message, code, prefix=b""):
    proc.kill()
    out, err = proc.communicate()
    close_master()
    sys.stderr.write(message + "\n")
    sys.stderr.buffer.write(prefix)
    sys.stderr.buffer.write(out)
    sys.stderr.buffer.write(err)
    sys.exit(code)

ready, _, _ = select.select([proc.stdout], [], [], {timeout_secs})
mode = "remote PTY" if use_pty else "remote"
if not ready:
    kill_and_fail(f"timed out while waiting for {{mode}} command readiness", 124)

line = proc.stdout.readline()
if b"ready" not in line:
    kill_and_fail(f"unexpected readiness line: {{line!r}}", 1)

os.kill(proc.pid, signal.SIGINT)
try:
    out, err = proc.communicate(timeout={timeout_secs})
except subprocess.TimeoutExpired:
    kill_and_fail(f"timed out waiting for biwa to forward SIGINT into the {{mode}} command", 124, line)

close_master()
combined = line + out
sys.stdout.buffer.write(combined)
sys.stderr.buffer.write(err)
if b"interrupted" not in combined:
    sys.stderr.write(f"{{mode}} command was not interrupted\n")
    sys.exit(1)
if proc.returncode == 0:
    sys.stderr.write(f"biwa exited successfully after {{mode}} SIGINT\n")
    sys.exit(1)
sys.exit(0)
"#,
		biwa_path = env!("CARGO_BIN_EXE_biwa"),
		remote_script = SIGINT_REMOTE_SCRIPT,
		timeout_secs = timeout_secs,
		use_pty = use_pty,
	);

	let output = Command::new("python3")
		.arg("-c")
		.arg(&python)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
		.output()?;

	assert!(
		output.status.success(),
		"stdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(
		String::from_utf8_lossy(&output.stdout).contains("interrupted"),
		"stdout: {}",
		String::from_utf8_lossy(&output.stdout)
	);
	Ok(())
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
fn e2e_explicit_public_key_authentication() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");
	write_test_ssh_private_key(&key_path)?;
	let output = biwa_public_key_cmd(
		&["run", "--skip-sync", "echo", "public-key-success"],
		&key_path,
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(String::from_utf8_lossy(&output.stdout).contains("public-key-success"));
	Ok(())
}

#[test]
fn e2e_agent_public_key_authentication() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let auth_sock = dir.path().join("agent.sock");
	let unauthorized_key_path = dir.path().join("unauthorized_ed25519");
	let key_path = dir.path().join("id_ed25519");
	write_ssh_private_key_from_seed(&unauthorized_key_path, &[0x11; 32])?;
	write_test_ssh_private_key(&key_path)?;

	let child = Command::new("ssh-agent")
		.args([OsStr::new("-D"), OsStr::new("-a"), auth_sock.as_os_str()])
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()?;
	let _guard = ChildGuard(child);
	let deadline = Instant::now() + Duration::from_secs(5);
	while !auth_sock.exists() {
		if Instant::now() >= deadline {
			return Err(eyre!("timed out waiting for test SSH agent"));
		}
		thread::sleep(Duration::from_millis(10));
	}

	let add = Command::new("ssh-add")
		.args([&unauthorized_key_path, &key_path])
		.env("SSH_AUTH_SOCK", &auth_sock)
		.output()?;
	assert!(
		add.status.success(),
		"ssh-add stderr: {}",
		String::from_utf8_lossy(&add.stderr)
	);

	let output = biwa_agent_cmd(&["run", "--skip-sync", "echo", "agent-success"], &auth_sock)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(String::from_utf8_lossy(&output.stdout).contains("agent-success"));
	Ok(())
}

#[test]
fn e2e_accept_new_then_strict_host_key_checking() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let known_hosts = dir.path().join("known_hosts");

	let first = biwa_host_key_cmd(&["run", "--skip-sync", "true"], "accept-new", &known_hosts)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		first.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&first.stderr)
	);
	assert!(known_hosts.is_file());

	let strict = biwa_host_key_cmd(&["run", "--skip-sync", "true"], "strict", &known_hosts)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		strict.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&strict.stderr)
	);
	Ok(())
}

#[test]
fn e2e_strict_rejects_unknown_host_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let output = biwa_host_key_cmd(
		&["run", "--skip-sync", "true"],
		"strict",
		&dir.path().join("known_hosts"),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("Unknown SSH host key"), "stderr: {stderr}");
	Ok(())
}

#[test]
fn e2e_insecure_accepts_without_learning_host_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let known_hosts = dir.path().join("known_hosts");
	let output = biwa_host_key_cmd(&["run", "--skip-sync", "true"], "insecure", &known_hosts)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success(), "stderr: {stderr}");
	assert!(!known_hosts.exists());
	assert!(
		stdout.contains("SSH host key verification is disabled"),
		"stdout: {stdout}"
	);
	Ok(())
}

#[test]
fn e2e_run_pull_round_trip_applies_remote_results() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;

	let output = biwa_cmd(&[
		"run",
		"--pull",
		"sh",
		"-c",
		"test \"$(cat input.txt)\" = local && rm input.txt && printf remote > result.txt",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success(), "stderr: {stderr}");
	assert!(!dir.path().join("input.txt").exists());
	pretty_assertions::assert_eq!(fs::read_to_string(dir.path().join("result.txt"))?, "remote");
	assert!(stderr.contains("Push completed"), "stderr: {stderr}");
	assert!(stderr.contains("Pull completed"), "stderr: {stderr}");
	Ok(())
}

#[test]
fn e2e_run_pull_skips_pull_after_nonzero_exit() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let local_root = dir.path().join("project");
	fs::create_dir_all(&local_root)?;
	fs::write(local_root.join("input.txt"), "local")?;
	fs::create_dir_all(local_root.join("ignored"))?;
	fs::write(local_root.join("ignored/keep.txt"), "keep local")?;
	let remote_dir = format!("{}-recovery", common::get_remote_project_dir(&local_root)?);

	let output = biwa_cmd(&[
		"run",
		"--pull",
		"--sync-root",
		local_root
			.to_str()
			.ok_or_else(|| eyre!("non-UTF-8 test path"))?,
		"--remote-dir",
		&remote_dir,
		"--force",
		"--include",
		"project/**",
		"--exclude",
		"project/ignored/**",
		"sh",
		"-c",
		"printf partial > result.txt; exit 7",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(!local_root.join("result.txt").exists());
	assert!(
		stderr.contains("results were not pulled"),
		"stderr: {stderr}"
	);
	assert!(stderr.contains(&remote_dir), "stderr: {stderr}");
	assert!(
		stderr.contains(&local_root.display().to_string()),
		"stderr: {stderr}"
	);
	assert!(
		stderr.contains("To recover the exact resolved transfer scope"),
		"stderr: {stderr}"
	);
	assert!(stderr.contains("--force"), "stderr: {stderr}");
	assert!(
		stderr.contains(&dir.path().join("project/**").display().to_string()),
		"stderr: {stderr}"
	);
	assert!(
		stderr.contains(&dir.path().join("project/ignored/**").display().to_string()),
		"stderr: {stderr}"
	);

	let command_start = stderr
		.find("biwa pull --sync-root=")
		.ok_or_else(|| eyre!("recovery command missing from stderr: {stderr}"))?;
	let command = stderr
		.get(command_start..)
		.ok_or_else(|| eyre!("recovery command offset was not a character boundary"))?
		.lines()
		.next()
		.ok_or_else(|| eyre!("recovery command was empty"))?;
	let arguments = shell_words::split(command)?;
	if arguments.first().map(String::as_str) != Some("biwa") {
		return Err(eyre!("invalid recovery executable: {arguments:?}"));
	}
	let recovery_args: Vec<_> = arguments
		.get(1..)
		.ok_or_else(|| eyre!("recovery command had no arguments"))?
		.iter()
		.map(String::as_str)
		.collect();
	let recovery_cwd = tempfile::tempdir()?;
	let recovery = biwa_cmd(&recovery_args)
		.dir(recovery_cwd.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		recovery.status.success(),
		"recovery stderr: {}",
		String::from_utf8_lossy(&recovery.stderr)
	);
	pretty_assertions::assert_eq!(
		fs::read_to_string(local_root.join("result.txt"))?,
		"partial"
	);
	pretty_assertions::assert_eq!(
		fs::read_to_string(local_root.join("ignored/keep.txt"))?,
		"keep local"
	);
	Ok(())
}

#[test]
fn e2e_run_pull_round_trip_allows_sync_hooks() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;
	common::write_hooks_config(
		dir.path(),
		Some(r#"sh -c "printf pre > pre-marker.txt""#),
		Some(r#"sh -c "printf post > post-marker.txt""#),
	)?;

	let output = biwa_cmd(&[
		"run",
		"--pull",
		"sh",
		"-c",
		"test \"$(cat pre-marker.txt)\" = pre && printf remote > result.txt",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	// A post-sync hook that touches the sync root must not look like local drift.
	assert!(!stderr.contains("Local files changed"), "stderr: {stderr}");
	assert!(output.status.success(), "stderr: {stderr}");
	// The pre-sync hook's file was uploaded and survives the round trip.
	pretty_assertions::assert_eq!(
		fs::read_to_string(dir.path().join("pre-marker.txt"))?,
		"pre"
	);
	pretty_assertions::assert_eq!(fs::read_to_string(dir.path().join("result.txt"))?, "remote");
	// The post-sync hook ran after the push, so the pull mirrors its file away
	// like any other local-only entry in scope.
	assert!(!dir.path().join("post-marker.txt").exists());
	Ok(())
}

#[test]
fn e2e_run_pull_refuses_edits_made_while_the_post_sync_hook_ran() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;
	// The hook rewrites a file that already existed at push time, standing in for
	// any local edit landing during the hook's window. Only paths the hook *adds*
	// join the pull baseline, so this must still be reported as drift.
	common::write_hooks_config(
		dir.path(),
		None,
		Some(r#"sh -c "printf edited > input.txt""#),
	)?;

	let output = biwa_cmd(&["run", "--pull", "sh", "-c", "printf remote > input.txt"])
		.dir(dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("Local files changed"), "stderr: {stderr}");
	assert!(stderr.contains("input.txt"), "stderr: {stderr}");
	// The local edit survives instead of being overwritten by the remote copy.
	pretty_assertions::assert_eq!(fs::read_to_string(dir.path().join("input.txt"))?, "edited");
	Ok(())
}

#[test]
fn e2e_run_pull_always_allows_sync_hooks() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;
	common::write_hooks_config(
		dir.path(),
		None,
		Some(r#"sh -c "printf post > post-marker.txt""#),
	)?;

	let output = biwa_cmd(&[
		"run",
		"--pull-always",
		"sh",
		"-c",
		"printf partial > result.txt; exit 7",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!stderr.contains("Local files changed"), "stderr: {stderr}");
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("Pull completed"), "stderr: {stderr}");
	assert!(stderr.contains("exited with code 7"), "stderr: {stderr}");
	pretty_assertions::assert_eq!(
		fs::read_to_string(dir.path().join("result.txt"))?,
		"partial"
	);
	Ok(())
}

#[test]
fn e2e_run_pull_always_pulls_after_nonzero_exit() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;

	let output = biwa_cmd(&[
		"run",
		"--pull-always",
		"sh",
		"-c",
		"printf partial > result.txt; exit 7",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(
		fs::read_to_string(dir.path().join("result.txt"))?,
		"partial"
	);
	assert!(stderr.contains("Pull completed"), "stderr: {stderr}");
	assert!(stderr.contains("exited with code 7"), "stderr: {stderr}");
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_run_pull_always_pulls_after_signal_exit() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;

	let output = biwa_cmd(&[
		"run",
		"--pull-always",
		"sh",
		"-c",
		"printf partial > result.txt; kill -TERM $$",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(
		fs::read_to_string(dir.path().join("result.txt"))?,
		"partial"
	);
	assert!(stderr.contains("Pull completed"), "stderr: {stderr}");
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_run_pull_failure_preserves_local_and_remote_results() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;

	let output = biwa_cmd(&[
		"run",
		"--pull",
		"sh",
		"-c",
		"printf remote > result.txt && ln -s result.txt result-link",
	])
	.dir(dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("command succeeded, but pulling results"),
		"stderr: {stderr}"
	);
	pretty_assertions::assert_eq!(fs::read_to_string(dir.path().join("input.txt"))?, "local");
	assert!(!dir.path().join("result.txt").exists());
	assert!(!dir.path().join("result-link").exists());

	let recovery_check = biwa_cmd(&["run", "-d", &remote_dir, "test", "-L", "result-link"])
		.dir(dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		recovery_check.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&recovery_check.stderr)
	);
	Ok(())
}

#[test]
fn e2e_run_pull_rejects_local_edits_during_command() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let local_file = dir.path().join("shared.txt");
	fs::write(&local_file, "baseline")?;

	let mut child = biwa_process(&[
		"--quiet",
		"run",
		"--pull",
		"sh",
		"-c",
		"printf 'ready\\n'; sleep 0.5; printf remote > shared.txt",
	]);
	child
		.current_dir(dir.path())
		.env("BIWA_CLEAN_AUTO", "false")
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());
	let mut child = child.spawn()?;
	let stdout = child
		.stdout
		.take()
		.ok_or_else(|| eyre!("missing child stdout"))?;
	let mut stdout = BufReader::new(stdout);
	let mut ready = String::new();
	stdout.read_line(&mut ready)?;
	pretty_assertions::assert_eq!(ready, "ready\n");
	fs::write(&local_file, "local edit")?;

	let output = child.wait_with_output()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(fs::read_to_string(&local_file)?, "local edit");
	assert!(stderr.contains("Local files changed"), "stderr: {stderr}");
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_run_pull_sigint_during_staging_preserves_local_tree() -> Result<()> {
	use nix::sys::signal::{Signal, kill};
	use nix::unistd::Pid;
	use std::io::Result as IoResult;

	const FILE_COUNT: usize = 300;
	let dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	for index in 0..FILE_COUNT {
		fs::write(
			dir.path().join(format!("file-{index:04}.txt")),
			format!("local-{index:04}"),
		)?;
	}

	let mut child = biwa_process(&[
		"run",
		"--pull",
		"--remote-dir",
		&remote_dir,
		"sh",
		"-c",
		"for file in ./*.txt; do printf remote > \"$file\"; done",
	]);
	child
		.current_dir(dir.path())
		.env("BIWA_CLEAN_AUTO", "false")
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", FILE_COUNT.to_string())
		.stdout(Stdio::null())
		.stderr(Stdio::piped());
	let mut child = child.spawn()?;
	let deadline = Instant::now() + Duration::from_secs(20);
	loop {
		let staging_has_download = fs::read_dir(dir.path())?
			.filter_map(IoResult::ok)
			.filter(|entry| {
				entry
					.file_name()
					.to_string_lossy()
					.starts_with(".biwa-pull-stage-")
			})
			.any(|entry| {
				fs::read_dir(entry.path().join("downloads"))
					.is_ok_and(|mut entries| entries.next().is_some())
			});
		if staging_has_download {
			let pid = i32::try_from(child.id())?;
			kill(Pid::from_raw(pid), Signal::SIGINT)?;
			break;
		}
		if let Some(status) = child.try_wait()? {
			return Err(eyre!(
				"run --pull exited before staging could be interrupted: {status}"
			));
		}
		if Instant::now() >= deadline {
			child.kill()?;
			return Err(eyre!("timed out waiting for run --pull staging"));
		}
		thread::sleep(Duration::from_millis(1));
	}

	let output = child.wait_with_output()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("interrupted"), "stderr: {stderr}");
	for index in 0..FILE_COUNT {
		pretty_assertions::assert_eq!(
			fs::read_to_string(dir.path().join(format!("file-{index:04}.txt")))?,
			format!("local-{index:04}")
		);
	}
	assert!(
		!fs::read_dir(dir.path())?
			.filter_map(IoResult::ok)
			.any(|entry| entry
				.file_name()
				.to_string_lossy()
				.starts_with(".biwa-pull-stage-"))
	);
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_run_pull_preserves_local_symlink() -> Result<()> {
	use std::os::unix::fs::symlink;

	let dir = tempfile::tempdir()?;
	let outside = tempfile::tempdir()?;
	let link = dir.path().join("link");
	symlink(outside.path(), &link)?;

	let output = biwa_cmd(&["run", "--pull", "true"])
		.dir(dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success(), "stderr: {stderr}");
	assert!(fs::symlink_metadata(link)?.file_type().is_symlink());
	Ok(())
}

#[test]
fn e2e_run_pull_forces_push_for_explicit_remote_dir() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("input.txt"), "local")?;
	let remote_dir = format!("{}-round-trip", common::get_remote_project_dir(dir.path())?);

	let output = biwa_cmd(&[
		"run",
		"--pull",
		"--remote-dir",
		&remote_dir,
		"sh",
		"-c",
		"cat input.txt > result.txt",
	])
	.dir(dir.path())
	.env("BIWA_SYNC_AUTO", "false")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(fs::read_to_string(dir.path().join("result.txt"))?, "local");
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_run_pull_preserves_existing_local_file_mode() -> Result<()> {
	use std::os::unix::fs::PermissionsExt as _;

	let dir = tempfile::tempdir()?;
	let script = dir.path().join("script.sh");
	fs::write(&script, "before")?;
	fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;

	let output = biwa_cmd(&["run", "--pull", "sh", "-c", "printf after > script.sh"])
		.dir(dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(fs::read_to_string(&script)?, "after");
	pretty_assertions::assert_eq!(fs::metadata(script)?.permissions().mode() & 0o777, 0o755);
	Ok(())
}

#[test]
fn e2e_run_forwards_sigint_without_pty() -> Result<()> {
	run_sigint_forwarding_case(false)
}

#[test]
fn e2e_run_forwards_sigint_with_pty() -> Result<()> {
	run_sigint_forwarding_case(true)
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
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
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
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
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

#[test]
fn e2e_run_rejects_setenv_on_default_server() -> Result<()> {
	let output = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"BIWA_TEST_FLAG=ok",
		"printenv",
		"BIWA_TEST_FLAG",
	])
	.env("BIWA_ENV_FORWARD_METHOD", "setenv")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("rejected environment variable forwarding via setenv"),
		"stderr: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_run_forwards_setenv_on_capable_server() -> Result<()> {
	let output = biwa_cmd_capable(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--env",
		"BIWA_TEST_FLAG=ok",
		"printenv",
		"BIWA_TEST_FLAG",
	])
	.env("BIWA_ENV_FORWARD_METHOD", "setenv")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(stdout.trim(), "ok");
	Ok(())
}

/// A fake remote `mise` installed for one test and removed when the test ends.
///
/// The SSH test containers do not ship mise, so the wrapper is verified against
/// a stand-in that records how biwa invoked it and then execs the real command.
struct FakeMise {
	/// Remote directory holding the fake executable, relative to the home directory.
	dir_name: String,
	/// Whether the fake was installed on the capable SSH server.
	capable: bool,
}

impl FakeMise {
	/// Writes the fake executable into `~/<dir_name>` on the selected test server.
	fn install(dir_name: &str, capable: bool) -> Result<Self> {
		let fake = Self {
			dir_name: dir_name.to_owned(),
			capable,
		};
		let script = format!(
			"mkdir -p -- \"$HOME/{dir_name}\" && printf '%s\\n' \
			'#!/bin/sh' \
			'printf \"FAKE_MISE_ARGV:%s\\n\" \"$*\"' \
			'printf \"FAKE_MISE_ENV:%s\\n\" \"${{MISE_ENV-unset}}\"' \
			'if [ \"$1\" = exec ] || [ \"$1\" = x ]; then shift; fi' \
			'if [ \"$1\" = -- ]; then shift; fi' \
			'exec \"$@\"' \
			> \"$HOME/{dir_name}/mise\" && chmod +x \"$HOME/{dir_name}/mise\""
		);

		let output = fake
			.cmd(&["--quiet", "run", "--skip-sync", "sh", "-c", &script])
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()?;

		assert!(
			output.status.success(),
			"failed to install the fake remote mise: {}",
			String::from_utf8_lossy(&output.stderr)
		);
		Ok(fake)
	}

	/// Builds a biwa invocation against the server holding this fake mise.
	fn cmd(&self, args: &[&str]) -> duct::Expression {
		if self.capable {
			biwa_cmd_capable(args)
		} else {
			biwa_cmd(args)
		}
	}

	/// Returns the `~`-relative remote path of the fake executable.
	fn bin(&self) -> String {
		format!("~/{}/mise", self.dir_name)
	}

	/// Returns the fake executable as a shell word that expands `$HOME` remotely.
	fn shell_bin(&self) -> String {
		format!("\"$HOME\"/{}/mise", self.dir_name)
	}
}

impl Drop for FakeMise {
	fn drop(&mut self) {
		let script = format!("rm -rf -- \"$HOME/{}\"", self.dir_name);
		let cleanup = self
			.cmd(&["--quiet", "run", "--skip-sync", "sh", "-c", &script])
			.stdout_null()
			.stderr_null()
			.unchecked()
			.run();
		match cleanup {
			Ok(_) | Err(_) => {}
		}
	}
}

#[test]
fn e2e_run_mise_exec_mode_wraps_the_remote_command() -> Result<()> {
	let fake_mise = FakeMise::install(".biwa-test-mise-exec", false)?;

	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "echo", "mise wrapped"])
		.env("BIWA_MISE_ENABLED", "true")
		.env("BIWA_MISE_BIN", fake_mise.bin())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(
		stdout.contains("FAKE_MISE_ARGV:exec -- echo mise wrapped"),
		"stdout: {stdout}"
	);
	assert!(stdout.contains("FAKE_MISE_ENV:unset"), "stdout: {stdout}");
	assert!(stdout.contains("mise wrapped"), "stdout: {stdout}");
	Ok(())
}

#[test]
fn e2e_run_mise_command_prefix_and_env_selection() -> Result<()> {
	let fake_mise = FakeMise::install(".biwa-test-mise-prefix", false)?;

	// The availability check follows the prefix, so the unrelated default
	// `mise.bin` must not be probed here.
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "echo", "prefixed"])
		.env("BIWA_MISE_ENABLED", "true")
		.env("BIWA_MISE_MODE", "prefix")
		.env("BIWA_MISE_ENV", "dev")
		.env(
			"BIWA_MISE_COMMAND_PREFIX",
			format!("{} x --", fake_mise.shell_bin()),
		)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(
		stdout.contains("FAKE_MISE_ARGV:x -- echo prefixed"),
		"stdout: {stdout}"
	);
	assert!(stdout.contains("FAKE_MISE_ENV:dev"), "stdout: {stdout}");
	assert!(stdout.contains("prefixed"), "stdout: {stdout}");
	Ok(())
}

#[test]
fn e2e_run_mise_verifies_the_wrapper_in_the_command_context() -> Result<()> {
	let fake_mise = FakeMise::install(".biwa-test-mise-context", false)?;
	let remote_dir = format!("~/{}", fake_mise.dir_name);

	for prefix in [
		None,
		Some("./mise x --"),
		Some("MISE_ENV=inline ./mise x --"),
	] {
		let mut command = biwa_cmd(&[
			"--quiet",
			"run",
			"-d",
			&remote_dir,
			"--env",
			"PATH=.:/usr/bin:/bin",
			"echo",
			"context resolved",
		])
		.env("BIWA_MISE_ENABLED", "true");
		if let Some(prefix) = prefix {
			command = command.env("BIWA_MISE_COMMAND_PREFIX", prefix);
		}
		let output = command
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()?;
		let stderr = String::from_utf8_lossy(&output.stderr);
		let stdout = String::from_utf8_lossy(&output.stdout);
		assert!(
			output.status.success(),
			"prefix: {prefix:?}; stderr: {stderr}"
		);
		assert!(stdout.contains("context resolved"), "stdout: {stdout}");
	}
	Ok(())
}

#[test]
fn e2e_run_mise_probe_applies_setenv_before_resolving_the_working_directory() -> Result<()> {
	// The forwarded CDPATH makes `cd bin` enter /bin, where ./sh is available.
	let output = biwa_cmd_capable(&[
		"--quiet",
		"run",
		"-d",
		"bin",
		"--env",
		"CDPATH=/",
		"printf",
		"setenv-context",
	])
	.env("BIWA_MISE_ENABLED", "true")
	.env("BIWA_MISE_COMMAND_PREFIX", r#"./sh -c 'exec "$@"' biwa"#)
	.env("BIWA_ENV_FORWARD_METHOD", "setenv")
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	// Shell `cd` may print its destination when CDPATH is set.
	assert!(String::from_utf8_lossy(&output.stdout).ends_with("setenv-context"));
	Ok(())
}

#[test]
fn e2e_run_mise_forwards_env_with_setenv_method() -> Result<()> {
	let fake_mise = FakeMise::install(".biwa-test-mise-setenv", true)?;

	let output = fake_mise
		.cmd(&[
			"--quiet",
			"run",
			"--skip-sync",
			"--env",
			"BIWA_TEST_FLAG=ok",
			"printenv",
			"BIWA_TEST_FLAG",
		])
		.env("BIWA_ENV_FORWARD_METHOD", "setenv")
		.env("BIWA_MISE_ENABLED", "true")
		.env("BIWA_MISE_BIN", fake_mise.bin())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(
		stdout.contains("FAKE_MISE_ARGV:exec -- printenv BIWA_TEST_FLAG"),
		"stdout: {stdout}"
	);
	assert!(
		stdout.contains("ok"),
		"variables sent through setenv must reach the wrapped command; stdout: {stdout}"
	);
	Ok(())
}

#[test]
fn e2e_run_mise_missing_remote_binary_reports_setup_help() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "true"])
		.env("BIWA_MISE_ENABLED", "true")
		.env("BIWA_MISE_BIN", "biwa-missing-mise")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("`biwa-missing-mise` (from `mise.bin`) was not found on the remote host"),
		"stderr: {stderr}"
	);
	assert!(
		stderr.contains("BIWA_MISE_ENABLED=false biwa run --skip-sync"),
		"the suggested bootstrap must not be blocked by the same check; stderr: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_run_mise_missing_command_prefix_binary_is_reported_against_the_prefix() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "true"])
		.env("BIWA_MISE_ENABLED", "true")
		.env("BIWA_MISE_MODE", "prefix")
		.env("BIWA_MISE_COMMAND_PREFIX", "biwa-missing-mise x --")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("`biwa-missing-mise` (from `mise.command_prefix`) was not found"),
		"stderr: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_run_mise_without_verification_fails_when_the_wrapper_is_missing() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "true"])
		.env("BIWA_MISE_ENABLED", "true")
		.env("BIWA_MISE_VERIFY", "false")
		.env("BIWA_MISE_BIN", "biwa-missing-mise")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		!stderr.contains("was not found on the remote host"),
		"verification must be skipped entirely; stderr: {stderr}"
	);
	assert!(
		stderr.contains("exited with code 127"),
		"the remote shell should report the missing wrapper; stderr: {stderr}"
	);
	Ok(())
}

/// A project-local `[mise]` section must never choose what runs remotely.
///
/// Without the restriction, a cloned repository could ship `enabled = true` with
/// `bin = "sh"` plus its own `exec` file and have it executed on the user's SSH
/// host, because the wrapper runs after `cd` into the synced project directory.
#[test]
fn e2e_run_mise_section_from_project_config_cannot_hijack_remote_commands() -> Result<()> {
	let marker = ".biwa-test-mise-pwned";
	let dir = tempfile::tempdir()?;
	fs::write(
		dir.path().join("biwa.toml"),
		"[mise]\nenabled = true\nbin = \"sh\"\n",
	)?;
	fs::write(
		dir.path().join("exec"),
		format!("#!/bin/sh\ntouch \"$HOME/{marker}\"\n"),
	)?;

	// Sync stays enabled here, exactly as it would be for a freshly cloned repo.
	let output = biwa_cmd(&["--quiet", "run", "true"])
		.dir(dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("The [mise] section is not allowed in project-local configuration"),
		"stderr: {stderr}"
	);
	assert!(
		stderr.contains("mise.enabled, mise.bin"),
		"the error must name the offending keys; stderr: {stderr}"
	);

	let payload_check = biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"sh",
		"-c",
		&format!("test ! -e \"$HOME/{marker}\""),
	])
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		payload_check.status.success(),
		"the project-local [mise] payload must never run on the remote host"
	);
	Ok(())
}

#[test]
fn e2e_run_mise_disabled_by_default_leaves_commands_unwrapped() -> Result<()> {
	let output = biwa_cmd(&["--quiet", "run", "--skip-sync", "echo", "unwrapped"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	pretty_assertions::assert_eq!(stdout.trim(), "unwrapped");
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

#[cfg(unix)]
fn create_biwa_symlink(dir: &Path, name: &str) -> Result<PathBuf> {
	use std::os::unix::fs::symlink;

	let shim = dir.join(name);
	symlink(env!("CARGO_BIN_EXE_biwa"), &shim)?;
	Ok(shim)
}

#[cfg(unix)]
#[test]
fn e2e_direct_command_symlink_runs_allowed_remote_command() -> Result<()> {
	use std::os::unix::fs::PermissionsExt as _;

	let dir = tempfile::tempdir()?;
	let config_dir = tempfile::tempdir()?;
	let shim_dir = tempfile::tempdir()?;
	let remote_command = dir.path().join("1511");
	fs::write(
		dir.path().join("biwa.toml"),
		r#"
[env.vars]
PATH = ".:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
"#,
	)?;
	fs::create_dir_all(config_dir.path().join("biwa"))?;
	fs::write(
		config_dir.path().join("biwa/config.toml"),
		"[direct.commands]\n1511 = []\n",
	)?;
	fs::write(
		&remote_command,
		"#!/bin/sh\nprintf 'direct:%s:%s\\n' \"$1\" \"$2\"\n",
	)?;
	fs::set_permissions(&remote_command, fs::Permissions::from_mode(0o755))?;

	let shim = create_biwa_symlink(shim_dir.path(), "1511")?;
	let output = biwa_program_cmd(&shim, &["autotest", "lab01"])
		.dir(dir.path())
		.env("HOME", config_dir.path())
		.env("XDG_CONFIG_HOME", config_dir.path())
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout),
		"direct:autotest:lab01\n"
	);
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_direct_command_options_are_biwa_run_options() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let config_dir = tempfile::tempdir()?;
	let shim_dir = tempfile::tempdir()?;
	let remote_command = "biwa-remote-nosync";
	let remote_dir = format!(
		"/tmp/biwa-direct-default-args-{}",
		dir.path()
			.file_name()
			.ok_or_else(|| eyre!("tempdir had no file name"))?
			.to_string_lossy()
	);
	let setup_script = format!(
		r#"cat > {remote_command} <<'EOF'
#!/bin/sh
printf 'nosync:%s\n' "$PWD"
EOF
chmod +x {remote_command}
"#
	);
	biwa_cmd(&["--quiet", "run", "--skip-sync", "rm", "-rf", &remote_dir])
		.stdout_null()
		.stderr_null()
		.run()?;
	biwa_cmd(&[
		"--quiet",
		"run",
		"--skip-sync",
		"--remote-dir",
		&remote_dir,
		"sh",
		"-c",
		&setup_script,
	])
	.stdout_null()
	.stderr_capture()
	.run()?;
	fs::write(
		dir.path().join("biwa.toml"),
		r#"
[sync.sftp]
max_files_to_sync = 0

[env.vars]
PATH = ".:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
"#,
	)?;
	fs::create_dir_all(config_dir.path().join("biwa"))?;
	fs::write(
		config_dir.path().join("biwa/config.toml"),
		format!(
			"[direct.commands]\n{remote_command} = [\"--skip-sync\", \"--remote-dir\", \"{remote_dir}\"]\n"
		),
	)?;

	let shim = create_biwa_symlink(shim_dir.path(), remote_command)?;
	let output = biwa_program_cmd(&shim, &[])
		.dir(dir.path())
		.env("HOME", config_dir.path())
		.env("XDG_CONFIG_HOME", config_dir.path())
		.env("BIWA_LOG_QUIET", "true")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	pretty_assertions::assert_eq!(
		String::from_utf8_lossy(&output.stdout),
		format!("nosync:{remote_dir}\n")
	);
	biwa_cmd(&["--quiet", "run", "--skip-sync", "rm", "-rf", &remote_dir])
		.stdout_null()
		.stderr_null()
		.run()?;
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_direct_command_symlink_rejects_non_allowed_command() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let config_dir = tempfile::tempdir()?;
	let shim_dir = tempfile::tempdir()?;
	fs::write(
		dir.path().join("biwa.toml"),
		"[direct.commands]\nnot-allowed = []\n",
	)?;
	fs::create_dir_all(config_dir.path().join("biwa"))?;
	fs::write(
		config_dir.path().join("biwa/config.toml"),
		"[direct.commands]\n1511 = []\n",
	)?;

	let shim = create_biwa_symlink(shim_dir.path(), "not-allowed")?;
	let output = biwa_program_cmd(&shim, &[])
		.dir(dir.path())
		.env("HOME", config_dir.path())
		.env("XDG_CONFIG_HOME", config_dir.path())
		.env("BIWA_LOG_QUIET", "true")
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(!output.status.success());
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		stderr.contains("not configured in global `direct.commands`"),
		"stderr was: {stderr}"
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

	if fixture_name.starts_with("edge-mise-") {
		// These fixtures are copied in as a project-local config, where the whole
		// [mise] section is refused; it is only honored from global config.
		let stderr = String::from_utf8_lossy(&output.stderr);
		assert!(
			!output.status.success(),
			"fixture {}: expected a project-local [mise] section to be rejected",
			fixture.display()
		);
		assert!(
			stderr.contains("The [mise] section is not allowed in project-local configuration"),
			"fixture {}: expected a global-only config error, got: {stderr}",
			fixture.display()
		);
		return Ok(());
	}

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
