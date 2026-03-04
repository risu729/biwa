use std::process::Command;

#[test]
#[ignore = "requires running SSH server"]
fn test_e2e_run_command() {
	// We expect the SSH server to be running on 127.0.0.1:2222
	// If not, this test will fail. It's meant to be run in an environment
	// where the docker-compose or CI service is active.

	let mut cmd = Command::new(env!("CARGO_BIN_EXE_biwa"));

	cmd.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_LOG_QUIET", "true") // To make output cleaner
		.arg("run")
		.arg("echo")
		.arg("hello e2e from biwa");

	let output = cmd.output().expect("failed to execute process");

	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);

	println!("stdout: {}", stdout);
	println!("stderr: {}", stderr);

	assert!(
		output.status.success(),
		"Command failed with status: {}, stderr: {}",
		output.status,
		stderr
	);
	assert!(stdout.contains("hello e2e from biwa"));
}
