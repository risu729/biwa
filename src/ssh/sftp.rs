use crate::Result;
use crate::config::types::SftpPermissions;
use color_eyre::eyre::Context as _;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{BufReader, copy};
use tracing::{debug, warn};

/// Uploads a file to a remote SFTP server using an existing session.
/// We provide our own upload method because `async-ssh2-tokio`'s `upload_file`
/// creates a new channel for every file and does not allow specifying file attributes (like permissions)
/// atomically on creation, leading to race conditions where sensitive files might be readable.
pub(super) async fn upload_file(
	sftp: &SftpSession,
	local_path: &Path,
	remote_path: &str,
	secure_mode: u32,
	permissions: &SftpPermissions,
) -> Result<()> {
	let perm_attrs = FileAttributes {
		permissions: Some(secure_mode | 0x8000), // S_IFREG | permission bits
		..Default::default()
	};

	let mut local_file = File::open(local_path)
		.await
		.wrap_err_with(|| format!("Failed to open local file: {}", local_path.display()))?;
	let mut local_file_buffered = BufReader::new(&mut local_file);

	if matches!(permissions, SftpPermissions::Recreate) {
		let should_remove = sftp
			.metadata(remote_path)
			.await
			.map(|attrs| {
				attrs
					.permissions
					.map_or_else(|| true, |p| (p & 0o777) != secure_mode)
			})
			.unwrap_or(true); // Default to true if metadata fails
		if should_remove && let Err(e) = sftp.remove_file(remote_path).await {
			debug!(error = %e, path = remote_path, "Failed to remove pre-existing file or file did not exist");
		}
	}

	let open_flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
	let mut remote_file = sftp
		.open_with_flags_and_attributes(remote_path, open_flags, perm_attrs.clone())
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;

	if matches!(permissions, SftpPermissions::Setstat)
		&& let Err(e) = remote_file.set_metadata(perm_attrs).await
	{
		warn!(
			error = %e,
			path = remote_path,
			"Failed to enforce file permissions via fsetstat. \
			 Consider setting `sync.sftp.permissions = \"recreate\"` in your config."
		);
	}

	copy(&mut local_file_buffered, &mut remote_file)
		.await
		.wrap_err("Failed to write to remote file")?;

	Ok(())
}
