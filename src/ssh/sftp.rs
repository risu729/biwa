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
	// Include S_IFREG (0x8000) for file creation — required by open_with_flags_and_attributes
	let create_attrs = FileAttributes {
		permissions: Some(secure_mode | 0x8000),
		..Default::default()
	};
	// Permission-only attributes for setstat/fsetstat — file type bits must NOT be included
	let perm_attrs = FileAttributes {
		permissions: Some(secure_mode),
		..Default::default()
	};

	let mut local_file = File::open(local_path)
		.await
		.wrap_err_with(|| format!("Failed to open local file: {}", local_path.display()))?;
	let mut local_file_buffered = BufReader::new(&mut local_file);

	let mut remote_file = sftp
		.open_with_flags_and_attributes(
			remote_path,
			OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ,
			create_attrs,
		)
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;

	// Explicitly set permissions after opening to ensure they are strictly enforced
	// even if the file was pre-existing (which causes `open_with_flags_and_attributes` to ignore `attrs`).
	// This is a best-effort fallback: some SFTP servers (e.g. OpenSSH's internal-sftp) may reject
	// fsetstat/setstat. Since permissions are already set atomically during creation, a failure here
	// only affects pre-existing files being overwritten.
	if let Err(e) = remote_file.set_metadata(perm_attrs.clone()).await {
		tracing::debug!(error = %e, "Failed to fsetstat, falling back to setstat on session");
		if let Err(e2) = sftp.set_metadata(remote_path, perm_attrs).await {
			tracing::warn!(error = %e2, path = remote_path, "Failed to enforce file permissions via setstat; permissions were set during file creation");
		}
	}

	copy(&mut local_file_buffered, &mut remote_file)
		.await
		.wrap_err("Failed to write to remote file")?;

	Ok(())
}
