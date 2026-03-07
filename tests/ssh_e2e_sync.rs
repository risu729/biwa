#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]

use std::{fs, path::Path};

mod common;
use common::Result;

fn biwa_cmd(args: &[&str], current_dir: &Path) -> duct::Expression {
	common::biwa_cmd(args)
		.env("BIWA_SYNC_REMOTE_ROOT", "/config/cache/biwa/projects")
		.dir(current_dir)
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_basic() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "world")?;

	// Explicit sync
	let output = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Run with auto sync
	let output2 = biwa_cmd(
		&["run", "cat", "/config/cache/biwa/projects/hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let _stdout2 = String::from_utf8_lossy(&output2.stdout);
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let output3 = biwa_cmd(
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
fn e2e_sync_cleaning() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("to_delete.txt");
	fs::write(&file_path, "delete me")?;

	let output = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"));

	fs::remove_file(&file_path)?;

	let output2 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr2 = String::from_utf8_lossy(&output2.stderr);
	assert!(stderr2.contains("1 deleted"));
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let dir_path = dir.path().join("subdir");
	fs::create_dir_all(&dir_path)?;
	fs::write(dir_path.join("secret.txt"), "secret")?;

	// Create an executable file
	let script_path = dir_path.join("script.sh");
	fs::write(&script_path, "#!/bin/sh\necho hi")?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		let mut perms = fs::metadata(&script_path)?.permissions();
		perms.set_mode(0o755); // rwxr-xr-x
		fs::set_permissions(&script_path, perms)?;
	}

	let output = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(output.status.success());

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_dir = format!("{remote_proj_dir}/subdir");

	let ls_output = biwa_cmd(&["run", "ls", "-ld", &remote_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(ls_stdout.contains("drwx------"), "stdout: {ls_stdout}");

	let remote_file = format!("{remote_dir}/secret.txt");
	let ls_file_output = biwa_cmd(&["run", "ls", "-l", &remote_file], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_file_stdout = String::from_utf8_lossy(&ls_file_output.stdout);
	assert!(
		ls_file_stdout.contains("-rw-------"),
		"stdout: {ls_file_stdout}"
	);

	let remote_script = format!("{remote_dir}/script.sh");
	let ls_script_output = biwa_cmd(&["run", "ls", "-l", &remote_script], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_script_stdout = String::from_utf8_lossy(&ls_script_output.stdout);
	assert!(
		ls_script_stdout.contains("-rwx------"),
		"stdout: {ls_script_stdout}"
	);
	Ok(())
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_hashing() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hash.txt");
	fs::write(&file_path, "initial")?;

	let output1 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	fs::write(&file_path, "modified")?;

	let output2 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output2.stderr).contains("1 uploaded"));
	assert!(String::from_utf8_lossy(&output2.stderr).contains("0 unchanged"));

	// Unchanged
	let output3 = biwa_cmd(&["sync"], dir.path())
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
	let output = biwa_cmd(&["sync"], dir.path())
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

	let output = biwa_cmd(&["sync"], dir.path())
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
fn e2e_sync_force() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("file.txt"), "content")?;

	let output1 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;
	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	let output2 = biwa_cmd(&["sync", "--force"], dir.path())
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

	let output = biwa_cmd(&["sync"], dir.path())
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
	biwa_cmd(
		&[
			"run",
			"sh",
			"-c",
			&format!("mkdir -p '{dummy_dir}' && ln -s '{dummy_dir}' '{remote_dir}'"),
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;

	// Now try to run sync, it should fail
	fs::write(dir.path().join("test.txt"), "test")?;
	let output = biwa_cmd(&["sync"], dir.path())
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
	let malicious_name = "test_dir_$(echo injection_attempt)_'\"`\\";
	let proj_dir = base_dir.path().join(malicious_name);
	fs::create_dir_all(&proj_dir)?;
	fs::write(proj_dir.join("test.txt"), "content")?;

	// Sync should work correctly despite the malicious project name
	let output = biwa_cmd(&["sync"], &proj_dir)
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Compute unique project name
	let remote_proj_dir = common::get_remote_project_dir(&proj_dir)?;
	let remote_file = format!("{remote_proj_dir}/test.txt");

	let output_cat = biwa_cmd(&["run", "cat", &remote_file], &proj_dir)
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
