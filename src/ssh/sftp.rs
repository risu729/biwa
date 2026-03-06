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
	let attrs = FileAttributes {
		permissions: Some(secure_mode | 0x8000), // Append S_IFREG (regular file)
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
			attrs.clone(),
		)
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;

	// Explicitly set permissions after opening to ensure they are strictly enforced
	// even if the file was pre-existing (which causes `open_with_flags_and_attributes` to ignore `attrs`)
	if let Err(e) = remote_file.set_metadata(attrs.clone()).await {
		tracing::debug!(error = %e, "Failed to fsetstat, falling back to setstat on session");
		sftp.set_metadata(remote_path, attrs)
			.await
			.wrap_err("Failed to enforce secure file permissions")?;
	}

	copy(&mut local_file_buffered, &mut remote_file)
		.await
		.wrap_err("Failed to write to remote file")?;

	Ok(())
}
