use eyre::{Context as _, ContextCompat as _, Result};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use std::path::Path;
use tokio::fs::File;
use tokio::io::BufReader;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Uploads a file to a remote SFTP server using an existing session.
/// We provide our own upload method because `async-ssh2-tokio`'s `upload_file`
/// creates a new channel for every file and does not allow specifying file attributes (like permissions)
/// atomically on creation, leading to race conditions where sensitive files might be readable.
pub async fn upload_file(
	sftp: &SftpSession,
	local_path: &Path,
	remote_path: &str,
	secure_mode: u32,
) -> Result<()> {
	let attrs = FileAttributes {
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
			attrs,
		)
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;

	let mut buffer = vec![0; 8192];

	loop {
		let n = local_file_buffered.read(&mut buffer).await?;
		if n == 0 {
			break;
		}
		remote_file
			.write_all(buffer.get(..n).wrap_err("Buffer slice out of bounds")?)
			.await
			.wrap_err("Failed to write to remote file")?;
	}

	Ok(())
}
