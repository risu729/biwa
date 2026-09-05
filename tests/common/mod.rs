#![allow(
	clippy::allow_attributes,
	reason = "May not be used in all integration tests."
)]

#[expect(
	clippy::disallowed_types,
	reason = "This is the Result type for integration tests."
)]
pub type Result<T> = color_eyre::Result<T>;

use gethostname::gethostname;
use russh::keys::{
	PrivateKey,
	ssh_key::{LineEnding, private::Ed25519Keypair},
};
use sha2::Digest as _;
use std::sync::LazyLock;
use std::{
	env, fs, iter,
	path::{Path, PathBuf},
};

/// Shared state directory for one test binary.
static TEST_STATE_DIR: LazyLock<tempfile::TempDir> =
	LazyLock::new(|| tempfile::tempdir().expect("create test state directory"));

/// Writes the deterministic Ed25519 private key authorized by the SSH test servers.
#[allow(
	dead_code,
	reason = "Only authentication integration tests need a private key."
)]
pub fn write_test_ssh_private_key(path: &Path) -> Result<()> {
	const SEED: [u8; 32] = [
		0x5a, 0x1d, 0x94, 0x0a, 0x2e, 0xff, 0x11, 0xac, 0x5b, 0xd8, 0xf8, 0xa0, 0x66, 0x3f, 0x53,
		0x7b, 0x4c, 0xb3, 0x45, 0xcf, 0xce, 0x5e, 0x8f, 0x13, 0xe0, 0xa4, 0x59, 0xa7, 0xae, 0x45,
		0xd7, 0x15,
	];
	write_ssh_private_key_from_seed(path, &SEED)
}

/// Writes a deterministic disposable SSH private key from a test-only seed.
#[allow(
	dead_code,
	reason = "Only authentication integration tests need generated private keys."
)]
pub fn write_ssh_private_key_from_seed(path: &Path, seed: &[u8; 32]) -> Result<()> {
	let private_key = PrivateKey::from(Ed25519Keypair::from_seed(seed));
	private_key.write_openssh_file(path, LineEnding::LF)?;
	Ok(())
}

/// Initializes the global testing environment.
///
/// This installs `color_eyre` for better panic reporting. It runs automatically
/// before any tests thanks to the `#[ctor::ctor(unsafe)]` attribute.
#[ctor::ctor(unsafe)]
fn init_test_env() {
	#[expect(
		clippy::unused_result_ok,
		reason = "Multiple tests may attempt to initialize the global error handler."
	)]
	color_eyre::install().ok();
}

/// Creates a `duct::Expression` to run the `biwa` CLI with standard SSH environment variables.
///
/// This is used heavily in end-to-end tests to supply valid dummy credentials
/// and host connection strings out-of-the-box.
///
/// Parallel tests share the same persisted connection state file; disable automatic background
/// cleanup so `biwa clean --auto` does not remove other tests' remote project directories.
/// The state directory is isolated from the developer's real XDG state.
pub fn biwa_cmd(args: &[&str]) -> duct::Expression {
	biwa_cmd_with_port(args, ssh_port())
}

fn biwa_cmd_with_port(args: &[&str], port: &str) -> duct::Expression {
	biwa_program_cmd_with_port(env!("CARGO_BIN_EXE_biwa"), args, port)
}

/// Creates a `duct::Expression` to run a biwa-compatible executable path.
#[allow(
	dead_code,
	reason = "Only some integration test binaries use direct executable helpers."
)]
pub fn biwa_program_cmd<T>(program: T, args: &[&str]) -> duct::Expression
where
	T: duct::IntoExecutablePath,
{
	biwa_program_cmd_with_port(program, args, ssh_port())
}

fn biwa_program_cmd_with_port<T>(program: T, args: &[&str], port: &str) -> duct::Expression
where
	T: duct::IntoExecutablePath,
{
	duct::cmd(program, args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", port)
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", test_known_hosts_path())
		.env("BIWA_CLEAN_AUTO", "false")
		.env("BIWA_STATE_DIR", TEST_STATE_DIR.path())
		.env("XDG_DATA_HOME", TEST_STATE_DIR.path())
}

/// Creates a `duct::Expression` to run the `biwa` CLI against the capable SSH server.
///
/// The capable server allows `BIWA_TEST_*` variables through SSH `setenv` and uses an
/// SFTP subsystem that supports permission updates for files owned by the test user.
#[allow(
	dead_code,
	reason = "Only some integration test binaries use the capable server."
)]
pub fn biwa_cmd_capable(args: &[&str]) -> duct::Expression {
	biwa_cmd_with_port(args, ssh_capable_port())
}

/// Returns the host port for the default SSH test server.
#[allow(
	dead_code,
	reason = "Only tests that construct commands directly need the port."
)]
pub fn ssh_port() -> &'static str {
	static PORT: LazyLock<String> =
		LazyLock::new(|| env::var("BIWA_TEST_SSH_PORT").unwrap_or_else(|_| "2222".to_owned()));
	PORT.as_str()
}

/// Returns the isolated known-hosts path shared by this test binary.
#[allow(
	dead_code,
	reason = "Only tests that construct commands directly need the path."
)]
pub fn test_known_hosts_path() -> &'static Path {
	static PATH: LazyLock<PathBuf> = LazyLock::new(|| TEST_STATE_DIR.path().join("known_hosts"));
	PATH.as_path()
}

/// Returns the host port for the capable SSH test server.
fn ssh_capable_port() -> &'static str {
	static PORT: LazyLock<String> = LazyLock::new(|| {
		env::var("BIWA_TEST_SSH_CAPABLE_PORT").unwrap_or_else(|_| "2223".to_owned())
	});
	PORT.as_str()
}

/// Writes isolated global hooks; set the child process `XDG_CONFIG_HOME` to the returned directory.
#[allow(
	dead_code,
	reason = "Only the sync hook integration tests configure hooks."
)]
pub fn write_hooks_config(
	pre_sync: Option<&str>,
	post_sync: Option<&str>,
) -> Result<tempfile::TempDir> {
	let config = iter::once("[hooks]".to_owned())
		.chain(pre_sync.map(|command| format!("pre_sync = '{command}'")))
		.chain(post_sync.map(|command| format!("post_sync = '{command}'")))
		.collect::<Vec<_>>()
		.join("\n");
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("biwa"))?;
	fs::write(dir.path().join("biwa/config.toml"), config + "\n")?;
	Ok(dir)
}

/// Computes the absolute path to the remote project directory.
///
/// Mimics how `biwa` internally generates a unique project directory string on the
/// remote server by taking the project directory name and appending an
/// 8-character hex slice of both the local hostname hash and the canonical
/// absolute path hash.
#[allow(dead_code, reason = "May not be used in all integration tests.")]
pub fn get_remote_project_dir(local_dir: &Path) -> Result<String> {
	let proj_name = local_dir
		.file_name()
		.ok_or_else(|| color_eyre::eyre::eyre!("no file name"))?
		.to_string_lossy();

	let host_hash = hex::encode(sha2::Sha256::digest(
		gethostname().to_string_lossy().as_bytes(),
	));

	let path_hash = hex::encode(sha2::Sha256::digest(
		local_dir.canonicalize()?.to_string_lossy().as_bytes(),
	));

	#[expect(
		clippy::string_slice,
		reason = "Hex encoded strings are strictly ASCII, slicing is safe"
	)]
	let unique_proj_name = format!("{}-{}-{}", proj_name, &host_hash[..8], &path_hash[..8]);
	Ok(format!("~/.cache/biwa/projects/{unique_proj_name}"))
}
