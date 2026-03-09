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
use sha2::Digest as _;
use std::path::Path;

/// Initializes the global testing environment.
///
/// This installs `color_eyre` for better panic reporting. It runs automatically
/// before any tests thanks to the `#[ctor::ctor]` attribute.
#[ctor::ctor]
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
pub fn biwa_cmd(args: &[&str]) -> duct::Expression {
	duct::cmd(env!("CARGO_BIN_EXE_biwa"), args)
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "2222")
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_PASSWORD", "password123")
}

/// Computes the absolute path to the remote project directory.
///
/// Mimics how `biwa` internally generates a unique project directory string on the
/// remote server by taking the project directory name and appending an
/// 8-character hex slice of the SHA-256 hash of its hostname and canonical
/// absolute path.
#[allow(dead_code, reason = "May not be used in all integration tests.")]
pub fn get_remote_project_dir(local_dir: &Path) -> Result<String> {
	let proj_name = local_dir
		.file_name()
		.ok_or_else(|| color_eyre::eyre::eyre!("no file name"))?
		.to_string_lossy();
	let mut hasher = sha2::Sha256::new();
	sha2::Digest::update(&mut hasher, gethostname().to_string_lossy().as_bytes());
	sha2::Digest::update(&mut hasher, [0]);
	sha2::Digest::update(
		&mut hasher,
		local_dir.canonicalize()?.to_string_lossy().as_bytes(),
	);
	let hash_hex = hex::encode(sha2::Digest::finalize(hasher));
	#[expect(
		clippy::string_slice,
		reason = "Hex encoded strings are strictly ASCII, slicing is safe"
	)]
	let unique_proj_name = format!("{}-{}", proj_name, &hash_hex[..8]);
	Ok(format!("~/.cache/biwa/projects/{unique_proj_name}"))
}
