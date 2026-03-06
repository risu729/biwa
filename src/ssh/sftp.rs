use crate::Result;
use crate::config::types::SftpPermissions;
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
	permissions: &SftpPermissions,
) -> Result<()> {
	let create_attrs = FileAttributes {
		permissions: Some(secure_mode | 0x8000), // S_IFREG | permission bits
		..Default::default()
	};

	let mut local_file = File::open(local_path)
		.await
		.wrap_err_with(|| format!("Failed to open local file: {}", local_path.display()))?;
	let mut local_file_buffered = BufReader::new(&mut local_file);

	let open_flags = match permissions {
		SftpPermissions::Recreate => {
			let mut needs_recreate = true;

			// Check existing file permissions
			if let Ok(attrs) = sftp.metadata(remote_path).await
				&& let Some(perms) = attrs.permissions {
					// Compare only the permission bits (mask 0o777)
					if (perms & 0o777) == secure_mode {
						needs_recreate = false;
					}
				}

			if needs_recreate {
				// Remove any pre-existing file first so that `open_with_flags_and_attributes` creates a
				// brand-new file with the requested permissions. Without this, OpenSSH ignores the attrs
				// on an existing file, which could leave sensitive files (e.g. .env) with overly broad
				// permissions from a previous upload or manual edit.
				if let Err(e) = sftp.remove_file(remote_path).await {
					tracing::debug!(error = %e, path = remote_path, "Failed to remove pre-existing file or file did not exist");
				}
			}
			OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ
		}
		SftpPermissions::Setstat => {
			OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ
		}
	};

	let mut remote_file = sftp
		.open_with_flags_and_attributes(remote_path, open_flags, create_attrs)
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;

	if *permissions == SftpPermissions::Setstat {
		let perm_attrs = FileAttributes {
			permissions: Some(secure_mode),
			..Default::default()
		};
		if let Err(e) = remote_file.set_metadata(perm_attrs.clone()).await {
			tracing::debug!(error = %e, "Failed to fsetstat, falling back to setstat on session");
			if let Err(e2) = sftp.set_metadata(remote_path, perm_attrs).await {
				tracing::warn!(
					error = %e2,
					path = remote_path,
					"Failed to enforce file permissions via setstat. \
					 Consider setting `sync.sftp.permissions = \"recreate\"` in your config."
				);
			}
		}
	}

	copy(&mut local_file_buffered, &mut remote_file)
		.await
		.wrap_err("Failed to write to remote file")?;

	Ok(())
}
