#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
mod common;

use color_eyre::eyre::eyre;
use common::{Result, biwa_cmd, ssh_port, test_known_hosts_path, write_ssh_private_key_from_seed};
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;

/// Runs the biwa CLI with public-key authentication for one specific key.
///
/// The environment is built from scratch because duct keeps the innermost value of a
/// variable set more than once.
fn biwa_key_auth_cmd(args: &[&str], key_path: &Path, state_dir: &Path) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "public-key")
		.env("BIWA_SSH_KEY_PATH", key_path)
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
		.env("BIWA_CLEAN_AUTO", "false")
		.env("BIWA_STATE_DIR", state_dir)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.stdin_null()
}

/// Runs `biwa setup-ssh` with the shared password-authentication environment.
fn setup_ssh(args: &[&str]) -> duct::Expression {
	let mut command = vec!["setup-ssh"];
	command.extend_from_slice(args);
	biwa_cmd(&command)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.stdin_null()
}

/// Reads the remote `authorized_keys` file through a password-authenticated session.
fn remote_authorized_keys() -> Result<String> {
	let output = biwa_cmd(&[
		"run",
		"--skip-sync",
		"--quiet",
		"cat",
		"~/.ssh/authorized_keys",
	])
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.stdin_null()
	.run()?;

	if !output.status.success() {
		return Err(eyre!(
			"failed to read the remote authorized_keys: {}",
			String::from_utf8_lossy(&output.stderr)
		));
	}
	Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Returns the algorithm and material of a locally generated public key.
fn public_key_material(private_key_path: &Path) -> Result<String> {
	let public_key = fs::read_to_string(format!("{}.pub", private_key_path.display()))?;
	let mut fields = public_key.split_whitespace();
	let (Some(algorithm), Some(material)) = (fields.next(), fields.next()) else {
		return Err(eyre!("generated public key is malformed: {public_key}"));
	};
	Ok(format!("{algorithm} {material}"))
}

#[test]
fn e2e_setup_ssh_generates_installs_and_verifies_a_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let first = setup_ssh(&[
		"--key-path",
		&key_path.to_string_lossy(),
		"--generate",
		"--key-type",
		"ed25519",
	])
	.run()?;
	let first_stderr = String::from_utf8_lossy(&first.stderr);

	assert!(first.status.success(), "setup-ssh failed: {first_stderr}");
	assert!(
		first_stderr.contains("Generated an Ed25519 key pair"),
		"stderr was: {first_stderr}"
	);
	assert!(
		first_stderr.contains("Added the public key"),
		"stderr was: {first_stderr}"
	);
	assert!(
		first_stderr.contains("Key authentication works"),
		"stderr was: {first_stderr}"
	);
	assert!(key_path.is_file());
	assert!(key_path.with_extension("pub").is_file());

	// The generated key must authenticate on its own, without password fallback.
	let key_run = biwa_key_auth_cmd(
		&["run", "--skip-sync", "--quiet", "echo", "key-auth-ok"],
		&key_path,
		dir.path(),
	)
	.run()?;
	assert!(
		key_run.status.success(),
		"key authentication failed: {}",
		String::from_utf8_lossy(&key_run.stderr)
	);
	assert!(String::from_utf8_lossy(&key_run.stdout).contains("key-auth-ok"));

	// A second run must recognize the working key instead of installing it again.
	let second = setup_ssh(&["--key-path", &key_path.to_string_lossy()]).run()?;
	let second_stderr = String::from_utf8_lossy(&second.stderr);
	assert!(second.status.success(), "stderr was: {second_stderr}");
	assert!(
		second_stderr.contains("Key authentication works"),
		"stderr was: {second_stderr}"
	);

	let material = public_key_material(&key_path)?;
	let authorized_keys = remote_authorized_keys()?;
	assert_eq!(
		authorized_keys.matches(material.as_str()).count(),
		1,
		"the public key must be authorized exactly once: {authorized_keys}"
	);

	Ok(())
}

#[test]
fn e2e_setup_ssh_enforces_remote_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let output = setup_ssh(&["--key-path", &key_path.to_string_lossy(), "--generate"]).run()?;
	assert!(
		output.status.success(),
		"setup-ssh failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let permissions = biwa_cmd(&[
		"run",
		"--skip-sync",
		"--quiet",
		"sh",
		"-c",
		"ls -ld ~/.ssh; ls -l ~/.ssh/authorized_keys",
	])
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.stdin_null()
	.run()?;
	let listing = String::from_utf8_lossy(&permissions.stdout);

	assert!(permissions.status.success());
	assert!(
		listing.contains("drwx------"),
		"the remote ~/.ssh directory must be private: {listing}"
	);
	assert!(
		listing.contains("-rw-------"),
		"the remote authorized_keys file must be private: {listing}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_check_reports_an_unauthorized_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");
	write_ssh_private_key_from_seed(&key_path, &[0x2b; 32])?;

	let output = setup_ssh(&["--key-path", &key_path.to_string_lossy(), "--check"]).run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(
		!output.status.success(),
		"--check must fail for an unauthorized key: {stderr}"
	);
	assert!(
		stderr.contains("is not working yet"),
		"stderr was: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_check_reports_a_missing_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("missing_key");

	let output = setup_ssh(&["--key-path", &key_path.to_string_lossy(), "--check"]).run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr was: {stderr}");
	assert!(
		stderr.contains("No SSH private key exists"),
		"stderr was: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_requires_generate_without_a_terminal() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let output = setup_ssh(&["--key-path", &key_path.to_string_lossy()]).run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr was: {stderr}");
	assert!(stderr.contains("--generate"), "stderr was: {stderr}");
	assert!(!key_path.exists());
	Ok(())
}

#[test]
fn e2e_setup_ssh_writes_the_key_into_the_local_config() -> Result<()> {
	let project = tempfile::tempdir()?;
	let key_path = project.path().join("id_ed25519");
	let config_path = project.path().join("biwa.toml");
	fs::write(
		&config_path,
		"#:schema https://biwa.takuk.me/schema/config.json\n\n[ssh]\nhost = \"127.0.0.1\"\nauth = \"password\"\n",
	)?;

	let output = setup_ssh(&[
		"--key-path",
		&key_path.to_string_lossy(),
		"--generate",
		"--write-config",
	])
	.dir(project.path())
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "setup-ssh failed: {stderr}");

	let updated = fs::read_to_string(&config_path)?;
	assert!(
		updated.contains(&format!("key_path = \"{}\"", key_path.display())),
		"config was: {updated}"
	);
	assert!(
		updated.contains("auth = \"public-key\""),
		"config was: {updated}"
	);
	assert!(
		updated.starts_with("#:schema https://biwa.takuk.me/schema/config.json"),
		"config was: {updated}"
	);
	Ok(())
}
