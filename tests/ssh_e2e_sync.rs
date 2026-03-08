#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
#![expect(
	clippy::shadow_unrelated,
	reason = "some tests have repeated variable names"
)]

use color_eyre::eyre::eyre;
use common::Result;
use rstest::rstest;
use std::{fs, path::Path};

mod common;

fn biwa_cmd(args: &[&str], current_dir: &Path) -> duct::Expression {
	common::biwa_cmd(args).dir(current_dir)
}

fn biwa_cmd_tilde(args: &[&str], current_dir: &Path) -> duct::Expression {
	biwa_cmd(args, current_dir).env("BIWA_SYNC_REMOTE_ROOT", "~/.cache/biwa/projects")
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_basic() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "world")?;

	// Explicit sync
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Run with auto sync
	let output2 = biwa_cmd_tilde(
		&["run", "cat", "~/.cache/biwa/projects/hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let _stdout2 = String::from_utf8_lossy(&output2.stdout);
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let output3 = biwa_cmd_tilde(
		&["run", "cat", &format!("{remote_proj_dir}/hello.txt")],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout3 = String::from_utf8_lossy(&output3.stdout);
	assert!(output3.status.success());
	assert!(stdout3.contains("world"));
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_absolute_path() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("absolute.txt"), "hello absolute")?;

	// Explicit override of BIWA_SYNC_REMOTE_ROOT to an absolute path
	let output = biwa_cmd(&["sync"], dir.path())
		.env("BIWA_SYNC_REMOTE_ROOT", "/tmp/biwa_test_absolute")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"First sync failed: stderr: {stderr}"
	);
	assert!(
		stderr.contains("1 uploaded"),
		"First sync didn't upload: stderr: {stderr}"
	);

	// Run command and explicitly assert on the absolute path
	let proj_name_full = common::get_remote_project_dir(dir.path())?;
	let proj_name_suffix = proj_name_full
		.strip_prefix("~/.cache/biwa/projects/")
		.unwrap_or(&proj_name_full);

	let remote_file = format!("/tmp/biwa_test_absolute/{proj_name_suffix}/absolute.txt");
	let output2 = biwa_cmd(&["run", "cat", &remote_file], dir.path())
		.env("BIWA_SYNC_REMOTE_ROOT", "/tmp/biwa_test_absolute")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout2 = String::from_utf8_lossy(&output2.stdout);
	assert!(output2.status.success(), "cat failed for {remote_file}");
	assert!(stdout2.contains("hello absolute"));

	// Cleanup the absolute directory to be a good citizen
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is acceptable")]
	biwa_cmd(&["run", "rm", "-rf", "/tmp/biwa_test_absolute"], dir.path())
		.run()
		.ok();

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_cleaning() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("to_delete.txt");
	fs::write(&file_path, "delete me")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"));

	fs::remove_file(&file_path)?;

	let output2 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr2 = String::from_utf8_lossy(&output2.stderr);
	assert!(stderr2.contains("1 deleted"));
	Ok(())
}

#[rstest]
#[case::default(None, "drwx------", "-rw-------", "-rwx------", "-rw-------")]
#[case::umask_0077(Some("0077"), "drwx------", "-rw-------", "-rwx------", "-rw-------")]
#[case::umask_0022(Some("0022"), "drwxr-xr-x", "-rw-r--r--", "-rwxr-xr-x", "-rw-r--r--")]
#[case::umask_0027(Some("0027"), "drwxr-x---", "-rw-r-----", "-rwxr-x---", "-rw-r-----")]
#[ignore = "requires running SSH server"]
fn e2e_sync_permissions(
	#[case] umask: Option<&str>,
	#[case] expected_dir: &str,
	#[case] expected_secret: &str,
	#[case] expected_script: &str,
	#[case] expected_group: &str,
) -> Result<()> {
	let dir = tempfile::tempdir()?;
	let dir_path = dir.path().join("subdir");
	fs::create_dir_all(&dir_path)?;

	let secret_path = dir_path.join("secret.txt");
	fs::write(&secret_path, "secret")?;

	// Create an executable file
	let script_path = dir_path.join("script.sh");
	fs::write(&script_path, "#!/bin/sh\necho hi")?;

	let group_path = dir_path.join("group.txt");
	fs::write(&group_path, "group")?;

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;

		// 0775 for subdir
		let mut perms = fs::metadata(&dir_path)?.permissions();
		perms.set_mode(0o775);
		fs::set_permissions(&dir_path, perms)?;

		// 0644 for secret.txt (to verify permissive umask doesn't add perms)
		let mut perms = fs::metadata(&secret_path)?.permissions();
		perms.set_mode(0o644);
		fs::set_permissions(&secret_path, perms)?;

		// 0755 for script.sh
		let mut perms = fs::metadata(&script_path)?.permissions();
		perms.set_mode(0o755);
		fs::set_permissions(&script_path, perms)?;

		// 0664 for group.txt
		let mut perms = fs::metadata(&group_path)?.permissions();
		perms.set_mode(0o664);
		fs::set_permissions(&group_path, perms)?;
	}

	let run_cmd = |args: &[&str]| {
		let mut cmd = biwa_cmd_tilde(args, dir.path());
		if let Some(u) = umask {
			cmd = cmd.env("BIWA_SSH_UMASK", u);
		}
		cmd
	};

	let output = run_cmd(&["sync"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_dir = format!("{remote_proj_dir}/subdir");

	let ls_output = run_cmd(&["run", "ls", "-ld", &remote_dir])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(ls_stdout.contains(expected_dir), "dir stdout: {ls_stdout}");

	let remote_file = format!("{remote_dir}/secret.txt");
	let ls_file_output = run_cmd(&["run", "ls", "-l", &remote_file])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_file_stdout = String::from_utf8_lossy(&ls_file_output.stdout);
	assert!(
		ls_file_stdout.contains(expected_secret),
		"secret stdout: {ls_file_stdout}"
	);

	let remote_script = format!("{remote_dir}/script.sh");
	let ls_script_output = run_cmd(&["run", "ls", "-l", &remote_script])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_script_stdout = String::from_utf8_lossy(&ls_script_output.stdout);
	assert!(
		ls_script_stdout.contains(expected_script),
		"script stdout: {ls_script_stdout}"
	);

	let remote_group = format!("{remote_dir}/group.txt");
	let ls_group_output = run_cmd(&["run", "ls", "-l", &remote_group])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_group_stdout = String::from_utf8_lossy(&ls_group_output.stdout);
	assert!(
		ls_group_stdout.contains(expected_group),
		"group stdout: {ls_group_stdout}"
	);
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_hashing() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hash.txt");
	fs::write(&file_path, "initial")?;

	let output1 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	fs::write(&file_path, "modified")?;

	let output2 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output2.stderr).contains("1 uploaded"));
	assert!(String::from_utf8_lossy(&output2.stderr).contains("0 unchanged"));

	// Unchanged
	let output3 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output3.stderr).contains("0 uploaded"));
	assert!(String::from_utf8_lossy(&output3.stderr).contains("1 unchanged"));
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_abort() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("file1.txt"), "1")?;
	fs::write(dir.path().join("file2.txt"), "2")?;

	// Set max_files_to_sync to 1
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", "1")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains("Aborting synchronization: 2 files to upload exceeds the limit of 1."));
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_ignore_gitignore() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
	fs::write(dir.path().join("ignored.txt"), "this should not sync")?;
	fs::write(dir.path().join("kept.txt"), "this should sync")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 uploaded"), "stderr: {stderr}"); // kept.txt and .gitignore
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_ignore_biwaignore() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".biwaignore"), ".env\n")?;
	fs::write(dir.path().join(".env"), "SECRET=val")?;
	fs::write(dir.path().join("main.rs"), "fn main() {}")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 uploaded"), "stderr: {stderr}"); // main.rs and .biwaignore
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_exclude_globset() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let tests_dir = dir.path().join("tests");
	fs::create_dir_all(&tests_dir)?;
	fs::write(tests_dir.join("a.txt"), "a")?;
	fs::write(dir.path().join("b.txt"), "b")?;

	// Exclude tests directory relative to current cwd
	let output = biwa_cmd_tilde(&["sync", "--exclude", "tests/**"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}"); // Only b.txt
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_exclude_config() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(
		dir.path().join("biwa.toml"),
		"sync.exclude = [\"secret_*.txt\"]\n",
	)?;
	fs::write(dir.path().join("secret_a.txt"), "a")?;
	fs::write(dir.path().join("public.txt"), "b")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 uploaded"), "stderr: {stderr}"); // biwa.toml and public.txt
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_force() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("file.txt"), "content")?;

	let output1 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;
	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	let output2 = biwa_cmd_tilde(&["sync", "--force"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stderr2 = String::from_utf8_lossy(&output2.stderr);
	assert!(stderr2.contains("1 uploaded"), "stderr2: {stderr2}");
	assert!(stderr2.contains("0 unchanged"), "stderr2: {stderr2}");
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_large_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	// 1MB file
	let large_content = vec![b'a'; 1024 * 1024];
	fs::write(dir.path().join("large.bin"), &large_content)?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success());
	assert!(stderr.contains("1 uploaded"));
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_remote_symlink() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;

	// Create a secondary dummy project just to run setup commands without BIWA trying to CD
	// into `remote_dir` (which doesn't exist or is a symlink we are trying to create).
	let setup_dir = tempfile::tempdir()?;

	// Create a dummy dir to point the symlink to
	let dummy_dir = format!("{remote_dir}_dummy");
	biwa_cmd_tilde(
		&[
			"run",
			"sh",
			"-c",
			&format!("mkdir -p {dummy_dir} && ln -s {dummy_dir} {remote_dir}"),
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;

	// Now try to run sync, it should fail
	fs::write(dir.path().join("test.txt"), "test")?;
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		!output.status.success(),
		"Expected failure but succeeded.\nstdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		stderr
	);
	assert!(
		stderr.contains("remote directory is a symlink"),
		"stderr: {stderr}"
	);

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_shell_injection() -> Result<()> {
	let base_dir = tempfile::tempdir()?;
	let malicious_name = "test_dir_$(echo injection_attempt)_'\"`";
	let proj_dir = base_dir.path().join(malicious_name);
	fs::create_dir_all(&proj_dir)?;
	fs::write(proj_dir.join("test.txt"), "content")?;

	// Sync should work correctly despite the malicious project name
	let output = biwa_cmd_tilde(&["sync"], &proj_dir)
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Compute unique project name
	let remote_proj_dir = common::get_remote_project_dir(&proj_dir)?;
	let remote_file = format!("{remote_proj_dir}/test.txt");

	let output_cat = biwa_cmd_tilde(&["run", "cat", &remote_file], &proj_dir)
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stdout_cat = String::from_utf8_lossy(&output_cat.stdout);
	// BIWA CLI warnings might be logged to stdout in some configurations, so we check if it ends with or contains our expected text.
	assert!(
		stdout_cat.trim().ends_with("content"),
		"stdout: {stdout_cat}"
	);

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_intermediate_dir_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let deep_dir = dir.path().join("a").join("b").join("c");
	fs::create_dir_all(&deep_dir)?;
	fs::write(deep_dir.join("file.txt"), "hello")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	for path in ["", "/a", "/a/b", "/a/b/c"] {
		let remote_path = format!("{remote_proj_dir}{path}");
		let ls_output = biwa_cmd_tilde(&["run", "ls", "-ld", &remote_path], dir.path())
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()?;

		let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
		assert!(
			ls_output.status.success(),
			"ls failed for {remote_path}: {ls_stdout}\nstderr: {}",
			String::from_utf8_lossy(&ls_output.stderr)
		);
		assert!(
			ls_stdout.contains("drwx------"),
			"Directory {remote_path} does not have 0700 permissions. ls output: {ls_stdout}"
		);
	}

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_existing_dir_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let sub_dir = dir.path().join("preexisting");
	fs::create_dir_all(&sub_dir)?;
	fs::write(sub_dir.join("file.txt"), "hello")?;

	// 1. Manually create the remote directory with 0755 permissions
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_sub_dir = format!("{remote_proj_dir}/preexisting");

	// Ensure the base directory is created first and then the sub_dir with 0755
	biwa_cmd_tilde(
		&[
			"run",
			"sh",
			"-c",
			&format!("mkdir -p {remote_proj_dir} && mkdir -m 0755 {remote_sub_dir}"),
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	// 2. Sync the project
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	// 3. Verify that the permissions of the pre-existing directory were corrected to 0700
	let ls_output = biwa_cmd_tilde(&["run", "ls", "-ld", &remote_sub_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(
		ls_output.status.success(),
		"ls failed for {remote_sub_dir}: {ls_stdout}\nstderr: {}",
		String::from_utf8_lossy(&ls_output.stderr)
	);
	assert!(
		ls_stdout.contains("drwx------"),
		"Pre-existing directory {remote_sub_dir} was not corrected to 0700 permissions. ls output: {ls_stdout}"
	);

	// 4. Verify the project root itself was corrected to 0700
	let ls_root_output = biwa_cmd_tilde(&["run", "ls", "-ld", &remote_proj_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let ls_root_stdout = String::from_utf8_lossy(&ls_root_output.stdout);
	assert!(
		ls_root_stdout.contains("drwx------"),
		"Project root {remote_proj_dir} was not corrected to 0700 permissions. ls output: {ls_root_stdout}"
	);

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_remote_dir() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "remote dir test")?;

	let test_id = dir
		.path()
		.file_name()
		.ok_or_else(|| eyre!("Failed to get test ID from path: {:?}", dir.path()))?
		.to_string_lossy();
	let remote_dir_path_string = format!("/tmp/biwa_test_remote_dir_{test_id}");
	let remote_dir_path = remote_dir_path_string.as_str();
	let output = biwa_cmd(&["sync", "-d", remote_dir_path], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	let output_cat = biwa_cmd(
		&["run", "-d", remote_dir_path, "cat", "hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout_cat = String::from_utf8_lossy(&output_cat.stdout);
	assert!(
		output_cat.status.success(),
		"cat failed, stderr: {}",
		String::from_utf8_lossy(&output_cat.stderr)
	);
	assert!(stdout_cat.contains("remote dir test"));

	// Cleanup
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is acceptable")]
	biwa_cmd(
		&["run", "--skip-sync", "rm", "-rf", remote_dir_path],
		dir.path(),
	)
	.run()
	.ok();

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_remote_dir_tilde() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "tilde test")?;

	let test_id = dir
		.path()
		.file_name()
		.ok_or_else(|| eyre!("Failed to get test ID from path: {:?}", dir.path()))?
		.to_string_lossy();
	let remote_dir_path_string = format!("~/biwa_test_tilde_{test_id}");
	let remote_dir_path = remote_dir_path_string.as_str();
	let output = biwa_cmd(&["sync", "-d", remote_dir_path], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	let output_cat = biwa_cmd(
		&["run", "-d", remote_dir_path, "cat", "hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout_cat = String::from_utf8_lossy(&output_cat.stdout);
	assert!(
		output_cat.status.success(),
		"cat failed, stderr: {}",
		String::from_utf8_lossy(&output_cat.stderr)
	);
	assert!(stdout_cat.contains("tilde test"));

	// Check that we didn't create a literal "~" directory
	let output_test = biwa_cmd(&["run", "--skip-sync", "test", "-d", "./~"], dir.path())
		.unchecked()
		.run()?;
	assert!(
		!output_test.status.success(),
		"Literal ~ directory was created!"
	);

	// Cleanup
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is acceptable")]
	biwa_cmd(
		&["run", "--skip-sync", "rm", "-rf", remote_dir_path],
		dir.path(),
	)
	.run()
	.ok();

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_remote_file_symlink_overwrite() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;

	// Create a secondary dummy project just to run setup commands without BIWA trying to CD
	// into `remote_dir`
	let setup_dir = tempfile::tempdir()?;

	// Create a dummy dir to point the file symlink to
	let dummy_dir = format!("{remote_dir}_dummy");
	biwa_cmd_tilde(
		&[
			"run",
			"sh",
			"-c",
			&format!(
				"mkdir -p {dummy_dir} && echo 'secret' > {dummy_dir}/sensitive.txt && \
				 mkdir -p {remote_dir} && ln -s {dummy_dir}/sensitive.txt {remote_dir}/test.txt"
			),
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;

	// Now try to run sync, it should succeed and replace the file symlink
	fs::write(dir.path().join("test.txt"), "overwritten")?;
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"Expected success but failed.\nstdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		stderr
	);
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Verify the original sensitive file was not overwritten
	let output_sensitive = biwa_cmd_tilde(
		&["run", "cat", &format!("{dummy_dir}/sensitive.txt")],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;
	let stdout_sensitive = String::from_utf8_lossy(&output_sensitive.stdout);
	assert!(
		stdout_sensitive.contains("secret"),
		"Sensitive file was overwritten or missing!"
	);

	// Verify the synced file is correct
	let output_synced = biwa_cmd_tilde(
		&["run", "cat", &format!("{remote_dir}/test.txt")],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;
	let stdout_synced = String::from_utf8_lossy(&output_synced.stdout);
	assert!(
		stdout_synced.contains("overwritten"),
		"Synced file content is incorrect: {stdout_synced}"
	);

	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_hidden_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".secret_config"), "my secret config")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	Ok(())
}
