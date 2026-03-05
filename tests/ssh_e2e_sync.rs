#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::unwrap_used, reason = "Tests can panic")]
#![expect(clippy::absolute_paths, reason = "Tests can use absolute paths")]
#![expect(clippy::create_dir, reason = "Tests can use create_dir")]
#![expect(
	clippy::string_slice,
	reason = "Hex encoded strings are strictly ASCII, slicing is safe"
)]

use sha2::Digest as _;
use std::fs;

fn biwa_cmd(args: &[&str], current_dir: &std::path::Path) -> duct::Expression {
	let mut biwa = duct::cmd(env!("CARGO_BIN_EXE_biwa"), args);
	biwa = biwa
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SYNC_REMOTE_ROOT", "/config/cache/biwa/projects")
		.dir(current_dir);
	biwa
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_basic() {
	let dir = tempfile::tempdir().unwrap();
	fs::write(dir.path().join("hello.txt"), "world").unwrap();

	// Explicit sync
	let output = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.expect("failed to execute process");

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
	.run()
	.expect("failed to execute process");

	let _stdout2 = String::from_utf8_lossy(&output2.stdout);
	// Wait, the remote path includes the project name. The project name is the directory name.
	// We don't know the tempdir name.
	let proj_name = dir.path().file_name().unwrap().to_string_lossy();
	let mut hasher = sha2::Sha256::new();
	sha2::Digest::update(
		&mut hasher,
		dir.path()
			.canonicalize()
			.unwrap()
			.to_string_lossy()
			.as_bytes(),
	);
	let hash_hex = hex::encode(sha2::Digest::finalize(hasher));
	let unique_proj_name = format!("{}-{}", proj_name, &hash_hex[..8]);

	let output3 = biwa_cmd(
		&[
			"run",
			"cat",
			&format!("/config/cache/biwa/projects/{unique_proj_name}/hello.txt"),
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()
	.expect("failed to execute process");

	let stdout3 = String::from_utf8_lossy(&output3.stdout);
	assert!(output3.status.success());
	assert!(stdout3.contains("world"));
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_cleaning() {
	let dir = tempfile::tempdir().unwrap();
	let file_path = dir.path().join("to_delete.txt");
	fs::write(&file_path, "delete me").unwrap();

	let output = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.unwrap();

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"));

	fs::remove_file(&file_path).unwrap();

	let output2 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.unwrap();

	let stderr2 = String::from_utf8_lossy(&output2.stderr);
	assert!(stderr2.contains("1 deleted"));
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_permissions() {
	let dir = tempfile::tempdir().unwrap();
	let dir_path = dir.path().join("subdir");
	fs::create_dir(&dir_path).unwrap();
	fs::write(dir_path.join("secret.txt"), "secret").unwrap();

	// Create an executable file
	let script_path = dir_path.join("script.sh");
	fs::write(&script_path, "#!/bin/sh\necho hi").unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		let mut perms = fs::metadata(&script_path).unwrap().permissions();
		perms.set_mode(0o755); // rwxr-xr-x
		fs::set_permissions(&script_path, perms).unwrap();
	}

	let output = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.unwrap();

	assert!(output.status.success());

	let proj_name = dir.path().file_name().unwrap().to_string_lossy();
	let mut hasher = sha2::Sha256::new();
	sha2::Digest::update(
		&mut hasher,
		dir.path()
			.canonicalize()
			.unwrap()
			.to_string_lossy()
			.as_bytes(),
	);
	let hash_hex = hex::encode(sha2::Digest::finalize(hasher));
	let unique_proj_name = format!("{}-{}", proj_name, &hash_hex[..8]);

	let remote_dir = format!("/config/cache/biwa/projects/{unique_proj_name}/subdir");

	let ls_output = biwa_cmd(&["run", "ls", "-ld", &remote_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()
		.unwrap();

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(ls_stdout.contains("drwx------"), "stdout: {ls_stdout}");

	let remote_file = format!("{remote_dir}/secret.txt");
	let ls_file_output = biwa_cmd(&["run", "ls", "-l", &remote_file], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()
		.unwrap();

	let ls_file_stdout = String::from_utf8_lossy(&ls_file_output.stdout);
	assert!(
		ls_file_stdout.contains("-rw-------"),
		"stdout: {ls_file_stdout}"
	);

	let remote_script = format!("{remote_dir}/script.sh");
	let ls_script_output = biwa_cmd(&["run", "ls", "-l", &remote_script], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()
		.unwrap();

	let ls_script_stdout = String::from_utf8_lossy(&ls_script_output.stdout);
	assert!(
		ls_script_stdout.contains("-rwx------"),
		"stdout: {ls_script_stdout}"
	);
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_hashing() {
	let dir = tempfile::tempdir().unwrap();
	let file_path = dir.path().join("hash.txt");
	fs::write(&file_path, "initial").unwrap();

	let output1 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()
		.unwrap();

	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	fs::write(&file_path, "modified").unwrap();

	let output2 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()
		.unwrap();

	assert!(String::from_utf8_lossy(&output2.stderr).contains("1 uploaded"));
	assert!(String::from_utf8_lossy(&output2.stderr).contains("0 unchanged"));

	// Unchanged
	let output3 = biwa_cmd(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()
		.unwrap();

	assert!(String::from_utf8_lossy(&output3.stderr).contains("0 uploaded"));
	assert!(String::from_utf8_lossy(&output3.stderr).contains("1 unchanged"));
}

#[test]
#[ignore = "requires running SSH server"]
fn e2e_sync_abort() {
	let dir = tempfile::tempdir().unwrap();
	fs::write(dir.path().join("file1.txt"), "1").unwrap();
	fs::write(dir.path().join("file2.txt"), "2").unwrap();

	// Set max_files_to_sync to 1
	let output = biwa_cmd(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", "1")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()
		.unwrap();

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains("Aborting synchronization: 2 files to upload exceeds the limit of 1."));
}
