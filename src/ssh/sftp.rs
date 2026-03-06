use crate::Result;
use color_eyre::eyre::Context as _;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{BufReader, copy};

/// Uploads a file to a remote SFTP server using an existing session.
/// We provide our own upload method because `async-ssh2-tokio`'s `upload_file`
/// creates a new channel for every file and does not allow specifying file attributes (like permissions)
/// atomically on creation, leading to race conditions where sensitive files might be readable.
pub(super) async fn upload_file(
	sftp: &SftpSession,
	local_path: &Path,
	remote_path: &str,
	secure_mode: u32,
) -> Result<()> {
	let create_attrs = FileAttributes {
		permissions: Some(secure_mode | 0x8000), // S_IFREG | permission bits
		..Default::default()
	};

	let mut local_file = File::open(local_path)
		.await
		.wrap_err_with(|| format!("Failed to open local file: {}", local_path.display()))?;
	let mut local_file_buffered = BufReader::new(&mut local_file);

	// Remove any pre-existing file first so that `open_with_flags_and_attributes` creates a
	// brand-new file with the requested permissions. Without this, OpenSSH ignores the attrs
	// on an existing file and many SFTP servers reject setstat, which could leave sensitive
	// files (e.g. .env) with overly broad permissions from a previous upload or manual edit.
	if let Err(e) = sftp.remove_file(remote_path).await {
		tracing::debug!(error = %e, path = remote_path, "No pre-existing file to remove (expected for first upload)");
	}

	let mut remote_file = sftp
		.open_with_flags_and_attributes(
			remote_path,
			OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ,
			create_attrs,
		)
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;

	copy(&mut local_file_buffered, &mut remote_file)
		.await
		.wrap_err("Failed to write to remote file")?;

	Ok(())
}
