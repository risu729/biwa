#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
mod common;

use color_eyre::eyre::eyre;
use common::{
	Result, biwa_cmd, biwa_cmd_capable, ssh_port, test_known_hosts_path,
	write_ssh_private_key_from_seed, write_test_ssh_private_key,
};
use core::time::Duration;
use pretty_assertions::assert_eq;
use russh::keys::PrivateKey;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Instant;

/// SSH test server a command runs against.
#[derive(Debug, Clone, Copy)]
enum Server {
	/// The default, restrictive server.
	Default,
	/// The server that also allows `setenv` and SFTP permission updates.
	Capable,
}

/// Runs the biwa CLI against one test server with captured, non-interactive I/O.
fn biwa(server: Server, args: &[&str]) -> duct::Expression {
	match server {
		Server::Default => biwa_cmd(args),
		Server::Capable => biwa_cmd_capable(args),
	}
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.stdin_null()
}

/// Runs `biwa setup-ssh` with the shared password-authentication environment.
fn setup_ssh(server: Server, args: &[&str]) -> duct::Expression {
	let mut command = vec!["setup-ssh"];
	command.extend_from_slice(args);
	biwa(server, &command)
}

/// Runs one remote command through a password-authenticated session.
fn remote(server: Server, args: &[&str]) -> Result<Output> {
	let mut command = vec!["run", "--skip-sync", "--quiet"];
	command.extend_from_slice(args);
	let output = biwa(server, &command).run()?;
	if !output.status.success() {
		return Err(eyre!(
			"remote command {args:?} failed: {}",
			String::from_utf8_lossy(&output.stderr)
		));
	}
	Ok(output)
}

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

/// Runs the biwa CLI with an explicit password against the default server.
///
/// The environment is built from scratch because duct keeps the innermost value of a
/// variable set more than once.
fn biwa_password_cmd(args: &[&str], password: &str, state_dir: &Path) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", password)
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
		.env("BIWA_CLEAN_AUTO", "false")
		.env("BIWA_STATE_DIR", state_dir)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.stdin_null()
}

/// Reads the remote `authorized_keys` file.
fn remote_authorized_keys(server: Server) -> Result<String> {
	let output = remote(server, &["cat", "~/.ssh/authorized_keys"])?;
	Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Returns the algorithm and material of an `authorized_keys` line.
fn key_material(public_key_line: &str) -> Result<String> {
	let mut fields = public_key_line.split_whitespace();
	let (Some(algorithm), Some(material)) = (fields.next(), fields.next()) else {
		return Err(eyre!("malformed public key line: {public_key_line}"));
	};
	Ok(format!("{algorithm} {material}"))
}

/// Returns the algorithm and material of a locally generated public key.
fn public_key_material(private_key_path: &Path) -> Result<String> {
	let public_key = fs::read_to_string(format!("{}.pub", private_key_path.display()))?;
	key_material(&public_key)
}

/// Returns the algorithm and material derived from a private key file.
fn private_key_material(private_key_path: &Path) -> Result<String> {
	let private_key = PrivateKey::read_openssh_file(private_key_path)?;
	key_material(&private_key.public_key().to_openssh()?)
}

/// Removes an installed public key from the remote `authorized_keys` file on drop.
///
/// The test servers are shared, so an installed key would otherwise accumulate for every
/// run of the suite.
struct RemoteKeyGuard {
	/// Server holding the key.
	server: Server,
	/// Algorithm and material of the installed key.
	material: String,
}

impl RemoteKeyGuard {
	/// Registers a key for removal. The material never contains shell metacharacters.
	const fn new(server: Server, material: String) -> Self {
		Self { server, material }
	}
}

impl Drop for RemoteKeyGuard {
	fn drop(&mut self) {
		let script = format!(
			"{{ grep -v -F -e '{}' ~/.ssh/authorized_keys > ~/.ssh/.biwa-test-keys || true; }} && mv ~/.ssh/.biwa-test-keys ~/.ssh/authorized_keys",
			self.material
		);
		if let Err(error) = remote(self.server, &["sh", "-c", &script]) {
			eprintln!("failed to remove the installed test key: {error}");
		}
	}
}

/// Stops a spawned helper process when the test ends.
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

/// Starts an isolated SSH agent holding the key the test servers authorize.
fn start_agent_with_test_key(dir: &Path) -> Result<(ChildGuard, PathBuf)> {
	let auth_sock = dir.join("agent.sock");
	let key_path = dir.join("agent_key");
	write_test_ssh_private_key(&key_path)?;

	let guard = ChildGuard(
		Command::new("ssh-agent")
			.args([OsStr::new("-D"), OsStr::new("-a"), auth_sock.as_os_str()])
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()?,
	);
	let deadline = Instant::now()
		.checked_add(Duration::from_secs(5))
		.ok_or_else(|| eyre!("the agent startup deadline overflowed"))?;
	while !auth_sock.exists() {
		if Instant::now() >= deadline {
			return Err(eyre!("timed out waiting for the test SSH agent"));
		}
		thread::sleep(Duration::from_millis(10));
	}

	let add = Command::new("ssh-add")
		.arg(&key_path)
		.env("SSH_AUTH_SOCK", &auth_sock)
		.output()?;
	if !add.status.success() {
		return Err(eyre!(
			"ssh-add failed: {}",
			String::from_utf8_lossy(&add.stderr)
		));
	}
	// The agent holds the key, so the local copy must not be discoverable as a key file.
	fs::remove_file(&key_path)?;

	Ok((guard, auth_sock))
}

#[test]
fn e2e_setup_ssh_check_accepts_an_agent_identity() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let (_agent, auth_sock) = start_agent_with_test_key(dir.path())?;

	// A password-configured user with a working agent identity must not be told to create
	// a redundant key.
	let output = duct::cmd(
		env!("CARGO_BIN_EXE_biwa"),
		["setup-ssh", "--check"].as_slice(),
	)
	.env("BIWA_SSH_HOST", "127.0.0.1")
	.env("BIWA_SSH_PORT", ssh_port())
	.env("BIWA_SSH_USER", "testuser")
	.env("BIWA_SSH_AUTH", "password")
	.env("BIWA_SSH_USE_CONFIG", "false")
	.env("SSH_AUTH_SOCK", &auth_sock)
	.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
	.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
	.env("BIWA_CLEAN_AUTO", "false")
	.env("BIWA_STATE_DIR", dir.path())
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.stdin_null()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(
		output.status.success(),
		"an agent identity must count as working key authentication: {stderr}"
	);
	assert!(
		stderr.contains("already works with your existing SSH credentials"),
		"stderr was: {stderr}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_generates_installs_and_verifies_a_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let first = setup_ssh(
		Server::Default,
		&[
			"--key-path",
			&key_path.to_string_lossy(),
			"--generate",
			"--key-type",
			"ed25519",
		],
	)
	.run()?;
	let first_stderr = String::from_utf8_lossy(&first.stderr);
	let _guard = RemoteKeyGuard::new(Server::Default, public_key_material(&key_path)?);

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
	let second = setup_ssh(
		Server::Default,
		&["--key-path", &key_path.to_string_lossy()],
	)
	.run()?;
	let second_stderr = String::from_utf8_lossy(&second.stderr);
	assert!(second.status.success(), "stderr was: {second_stderr}");
	assert!(
		second_stderr.contains("Key authentication works"),
		"stderr was: {second_stderr}"
	);

	let material = public_key_material(&key_path)?;
	let authorized_keys = remote_authorized_keys(Server::Default)?;
	assert_eq!(
		authorized_keys.matches(material.as_str()).count(),
		1,
		"the public key must be authorized exactly once: {authorized_keys}"
	);

	Ok(())
}

#[test]
fn e2e_setup_ssh_reinstalls_without_duplicating_the_entry() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let installed = setup_ssh(
		Server::Capable,
		&["--key-path", &key_path.to_string_lossy(), "--generate"],
	)
	.run()?;
	let _guard = RemoteKeyGuard::new(Server::Capable, public_key_material(&key_path)?);
	assert!(
		installed.status.success(),
		"setup-ssh failed: {}",
		String::from_utf8_lossy(&installed.stderr)
	);

	// Loosening the remote directory permissions makes sshd ignore authorized_keys, so the
	// second run reaches the remote script again with the key already listed.
	remote(Server::Capable, &["chmod", "0777", "~/.ssh"])?;

	let second = setup_ssh(
		Server::Capable,
		&["--key-path", &key_path.to_string_lossy()],
	)
	.run()?;
	let second_stderr = String::from_utf8_lossy(&second.stderr);
	assert!(second.status.success(), "stderr was: {second_stderr}");
	assert!(
		second_stderr.contains("already in ~/.ssh/authorized_keys"),
		"the remote script must recognize the existing entry: {second_stderr}"
	);
	assert!(
		second_stderr.contains("Key authentication works"),
		"stderr was: {second_stderr}"
	);

	let material = public_key_material(&key_path)?;
	let authorized_keys = remote_authorized_keys(Server::Capable)?;
	assert_eq!(
		authorized_keys.matches(material.as_str()).count(),
		1,
		"a repeated installation must not duplicate the entry: {authorized_keys}"
	);

	let listing = remote(Server::Capable, &["ls", "-ld", "~/.ssh"])?;
	assert!(
		String::from_utf8_lossy(&listing.stdout).contains("drwx------"),
		"the remote script must restore private permissions"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_enforces_remote_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let output = setup_ssh(
		Server::Default,
		&["--key-path", &key_path.to_string_lossy(), "--generate"],
	)
	.run()?;
	let _guard = RemoteKeyGuard::new(Server::Default, public_key_material(&key_path)?);
	assert!(
		output.status.success(),
		"setup-ssh failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let permissions = remote(
		Server::Default,
		&["sh", "-c", "ls -ld ~/.ssh; ls -l ~/.ssh/authorized_keys"],
	)?;
	let listing = String::from_utf8_lossy(&permissions.stdout);

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

	let output = setup_ssh(
		Server::Default,
		&["--key-path", &key_path.to_string_lossy(), "--check"],
	)
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(
		!output.status.success(),
		"--check must fail for an unauthorized key: {stderr}"
	);
	assert!(
		stderr.contains("is not working yet"),
		"stderr was: {stderr}"
	);

	// Verification must not change the remote authorized_keys file.
	let material = private_key_material(&key_path)?;
	let authorized_keys = remote_authorized_keys(Server::Default)?;
	assert!(
		!authorized_keys.contains(&material),
		"--check must not install the key: {authorized_keys}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_check_reports_a_missing_key() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("missing_key");

	let output = setup_ssh(
		Server::Default,
		&["--key-path", &key_path.to_string_lossy(), "--check"],
	)
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr was: {stderr}");
	assert!(
		stderr.contains("No SSH key or agent identity is available"),
		"stderr was: {stderr}"
	);
	assert!(!key_path.exists());
	Ok(())
}

#[test]
fn e2e_setup_ssh_requires_generate_without_a_terminal() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let output = setup_ssh(
		Server::Default,
		&["--key-path", &key_path.to_string_lossy()],
	)
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "stderr was: {stderr}");
	assert!(stderr.contains("--generate"), "stderr was: {stderr}");
	assert!(!key_path.exists());
	Ok(())
}

#[test]
fn e2e_setup_ssh_fails_without_installing_on_a_wrong_password() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let key_path = dir.path().join("id_ed25519");

	let output = biwa_password_cmd(
		&[
			"setup-ssh",
			"--key-path",
			&key_path.to_string_lossy(),
			"--generate",
		],
		"definitely-not-the-password",
		dir.path(),
	)
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(
		!output.status.success(),
		"a wrong password must fail: {stderr}"
	);
	assert!(
		stderr.contains("password authentication"),
		"stderr was: {stderr}"
	);
	assert!(
		!stderr.contains("definitely-not-the-password"),
		"the password must never be printed: {stderr}"
	);

	let material = public_key_material(&key_path)?;
	let authorized_keys = remote_authorized_keys(Server::Default)?;
	assert!(
		!authorized_keys.contains(&material),
		"a failed connection must not install the key: {authorized_keys}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_writes_the_key_into_the_local_config() -> Result<()> {
	let project = tempfile::tempdir()?;
	let key_path = project.path().join("id_ed25519");
	let config_path = project.path().join("biwa.toml");
	fs::write(
		&config_path,
		"#:schema https://biwa.takuk.me/schema/config.json\n\n[ssh]\nhost = \"127.0.0.1\"\nauth = \"password\" # migrated by hand\n",
	)?;

	let output = setup_ssh(
		Server::Default,
		&[
			"--key-path",
			&key_path.to_string_lossy(),
			"--generate",
			"--write-config",
		],
	)
	.dir(project.path())
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	let _guard = RemoteKeyGuard::new(Server::Default, public_key_material(&key_path)?);
	assert!(output.status.success(), "setup-ssh failed: {stderr}");

	let updated = fs::read_to_string(&config_path)?;
	assert!(
		updated.contains(&format!("key_path = \"{}\"", key_path.display())),
		"config was: {updated}"
	);
	assert!(
		updated.contains("auth = \"public-key\" # migrated by hand"),
		"config was: {updated}"
	);
	assert!(
		updated.starts_with("#:schema https://biwa.takuk.me/schema/config.json"),
		"config was: {updated}"
	);
	Ok(())
}

#[test]
fn e2e_setup_ssh_succeeds_when_the_config_cannot_be_rewritten() -> Result<()> {
	let project = tempfile::tempdir()?;
	let key_path = project.path().join("id_ed25519");
	let config_path = project.path().join("biwa.toml");
	// An inline table cannot be extended by appending an `[ssh]` section.
	let original = "ssh = { host = \"127.0.0.1\" }\n";
	fs::write(&config_path, original)?;

	let output = setup_ssh(
		Server::Default,
		&[
			"--key-path",
			&key_path.to_string_lossy(),
			"--generate",
			"--write-config",
		],
	)
	.dir(project.path())
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	let _guard = RemoteKeyGuard::new(Server::Default, public_key_material(&key_path)?);

	assert!(
		output.status.success(),
		"a configuration biwa cannot rewrite must not fail the setup: {stderr}"
	);
	assert!(
		stderr.contains("Key authentication works"),
		"stderr was: {stderr}"
	);
	assert!(stderr.contains("Add this manually"), "stderr was: {stderr}");
	assert_eq!(
		fs::read_to_string(&config_path)?,
		original,
		"the configuration file must be left untouched"
	);
	Ok(())
}
