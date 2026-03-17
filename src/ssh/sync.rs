use super::exec::connect;
use crate::Result;
use crate::config::types::{Config, SftpPermissions, SyncEngine};
use crate::ui::create_spinner;
use async_ssh2_tokio::client::Client;
use color_eyre::eyre::{Context as _, ContextCompat as _, bail};
use console::style;
use gethostname::gethostname;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use indicatif::ProgressBar;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use tokio::fs::{File as AsyncFile, metadata};
use tokio::io::{BufReader as AsyncBufReader, copy as async_copy};
use tokio::task::spawn_blocking;
use tracing::{debug, info, warn};

/// Statistics for a synchronization operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
	/// Number of files uploaded.
	pub uploaded: usize,
	/// Number of files deleted.
	pub deleted: usize,
	/// Number of files unchanged.
	pub unchanged: usize,
}

/// A local file and its SHA-256 hash.
#[derive(Debug, Clone)]
pub(super) struct LocalFile {
	/// The relative path to the file from the project root.
	pub path: PathBuf,
	/// The SHA-256 hash of the file content.
	pub hash: String,
}

/// Options for a synchronization operation.
#[derive(Debug, Default, Clone)]
pub struct Options {
	/// Force synchronization of all files, ignoring incremental hash checks.
	pub force: bool,
	/// Exclude files matching these paths or globs.
	pub exclude: Vec<String>,
	/// Only synchronize files matching these paths or globs.
	pub include: Vec<String>,
}

/// Builds a `GlobSet` from a slice of pattern strings.
fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
	if patterns.is_empty() {
		return Ok(None);
	}
	let mut builder = GlobSetBuilder::new();
	for pattern in patterns {
		builder.add(Glob::new(pattern)?);
	}
	builder
		.build()
		.wrap_err("Failed to build glob set")
		.map(Some)
}

/// Checks if the remote root path is absolute and prints a warning.
pub(super) fn check_remote_root(remote_root: &Path) {
	if remote_root.is_absolute() {
		warn!(
			"Absolute remote_root path detected: {}. It is recommended to use a relative path starting with '~'.",
			remote_root.display()
		);
	}
}

/// Shell-quotes a remote path while preserving home directory expansion.
///
/// If the path starts with `~/`, the `~` is replaced with `$HOME` and placed
/// outside the quotes so the shell can expand it. Otherwise, the entire path
/// is quoted with `shell_words::quote`.
pub(super) fn shell_quote_path(path: &str) -> String {
	if path == "~" || path == "$HOME" {
		return "\"$HOME\"".to_owned();
	}
	if let Some(rest) = path
		.strip_prefix("~/")
		.or_else(|| path.strip_prefix("$HOME/"))
	{
		return format!("\"$HOME\"/{}", shell_words::quote(rest));
	}
	shell_words::quote(path).into_owned()
}

/// A wrapper around a hasher that implements `std::io::Write`.
struct HasherWriter<'a, H> {
	/// The underlying hasher instance.
	hasher: &'a mut H,
}

impl<H: sha2::Digest> io::Write for HasherWriter<'_, H> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.hasher.update(buf);
		Ok(buf.len())
	}
	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

/// Computes the SHA-256 hash of a file.
fn hash_file(path: &Path) -> Result<String> {
	let file = File::open(path)?;
	let mut reader = io::BufReader::new(file);
	let mut hasher = Sha256::new();
	io::copy(
		&mut reader,
		&mut HasherWriter {
			hasher: &mut hasher,
		},
	)
	.wrap_err_with(|| format!("Failed to hash file: {}", path.display()))?;
	Ok(hex::encode(hasher.finalize()))
}

/// Collects local files from the project root, respecting ignore rules.
pub(super) fn collect_local_files(
	root: &Path,
	config_exclude: &[String],
	options: &Options,
) -> Result<Vec<LocalFile>> {
	let mut builder = WalkBuilder::new(root);
	builder.standard_filters(true); // .gitignore, .ignore, etc.
	builder.add_custom_ignore_filename(".biwaignore");
	builder.hidden(false); // Include hidden files (e.g. .env, .gitignore)
	builder.require_git(false); // Respect .gitignore even outside of git repositories

	let combined_exclude = config_exclude
		.iter()
		.chain(options.exclude.iter())
		.map(ToString::to_string)
		.collect::<Vec<_>>();
	let exclude_globs = build_globset(&combined_exclude)?;
	let include_globs = build_globset(&options.include)?;

	let mut result = Vec::new();
	for entry in builder.build() {
		let entry = entry?;
		let path = entry.path();
		if path.is_file() {
			let relative = path.strip_prefix(root).wrap_err("Failed to strip prefix")?;
			let absolute_str = path.to_string_lossy().into_owned();

			if exclude_globs
				.as_ref()
				.is_some_and(|set| set.is_match(&absolute_str))
			{
				continue;
			}

			if include_globs
				.as_ref()
				.is_some_and(|set| !set.is_match(&absolute_str))
			{
				continue;
			}

			result.push(LocalFile {
				path: relative.to_path_buf(),
				hash: hash_file(path)?,
			});
		}
	}
	Ok(result)
}

/// Computes the absolute remote path for a given local file.
pub(super) fn compute_remote_path(
	remote_root: &Path,
	project_name: &str,
	relative: &Path,
) -> String {
	let mut path = remote_root.to_string_lossy().into_owned();
	if !path.ends_with('/') {
		path.push('/');
	}
	path.push_str(project_name);

	let rel_str = relative.to_string_lossy();
	if !rel_str.is_empty() {
		if !path.ends_with('/') && !rel_str.starts_with('/') {
			path.push('/');
		}
		path.push_str(&rel_str);
	}
	path
}

/// Computes a unique project name based on the hostname and project root's canonical path.
fn compute_unique_project_name(project_root: &Path) -> Result<String> {
	let project_name = project_root
		.file_name()
		.wrap_err("Invalid project root directory")?
		.to_string_lossy()
		.into_owned();

	// Create a unique hash based on the hostname and absolute path to prevent
	// collisions between projects with the same name across machines.
	let mut hasher = Sha256::new();
	hasher.update(gethostname().to_string_lossy().as_bytes());
	hasher.update([0]);
	hasher.update(
		project_root
			.canonicalize()
			.wrap_err("Failed to canonicalize project root")?
			.to_string_lossy()
			.as_bytes(),
	);
	let hash_hex = hex::encode(hasher.finalize());
	#[expect(
		clippy::string_slice,
		reason = "Hex encoded strings are strictly ASCII, slicing is safe"
	)]
	Ok(format!("{}-{}", project_name, &hash_hex[..8]))
}

/// Computes the remote directory path for a given project.
///
/// This is the directory where synced files are stored on the remote server.
pub fn compute_project_remote_dir(config: &Config, project_root: &Path) -> Result<String> {
	let unique_project_name = compute_unique_project_name(project_root)?;
	Ok(compute_remote_path(
		&config.sync.remote_root,
		&unique_project_name,
		Path::new(""),
	))
}

/// Fetches the SHA-256 hashes of the files currently in the remote directory.
async fn fetch_remote_hashes(
	client: &Client,
	config: &Config,
	remote_dir: &str,
) -> Result<HashMap<String, String>> {
	let quoted_remote_dir = shell_quote_path(remote_dir);
	let dir_mode = format!("{:04o}", 0o777 & !config.ssh.umask.as_u32());

	// Create remote dir with target permissions and fetch current hashes
	let script = format!(
		"umask {} && mkdir -p -- {quoted_remote_dir} && \
		 if [ -L {quoted_remote_dir} ]; then echo 'Error: remote directory is a symlink' >&2; exit 1; fi && \
		 cd -- {quoted_remote_dir} && \
		 (find . -type d -exec chmod {dir_mode} {{}} + || true) && \
		 (find . -type f -exec sha256sum {{}} + || true)",
		&config.ssh.umask
	);

	let result = client
		.execute(&script)
		.await
		.wrap_err("Failed to fetch remote state")?;

	if result.exit_status != 0 {
		let stderr = result.stderr.trim();
		if stderr.contains("remote directory is a symlink") {
			bail!("remote directory is a symlink");
		}
		bail!(
			"Remote script failed with code {}: {}",
			result.exit_status,
			stderr
		);
	}

	let output = result.stdout;

	Ok(parse_remote_hashes(&output))
}

/// Actions to perform during synchronization.
struct SyncActions {
	/// Files to upload to the remote server.
	to_upload: Vec<PathBuf>,
	/// Files to delete from the remote server.
	to_delete: Vec<String>,
}

/// Compares local files with remote hashes to determine which files need to be uploaded or deleted.
fn calculate_sync_actions(
	local_files: &[LocalFile],
	remote_hashes: &HashMap<String, String>,
	options: &Options,
) -> SyncActions {
	let mut to_upload = Vec::new();
	let mut local_paths_str = HashSet::new();

	for local_file in local_files {
		let rel_path_str = local_file.path.to_string_lossy().into_owned();
		local_paths_str.insert(rel_path_str.clone());

		if !options.force
			&& let Some(remote_hash) = remote_hashes.get(&rel_path_str)
			&& remote_hash == &local_file.hash
		{
			continue;
		}
		to_upload.push(local_file.path.clone());
	}

	let mut to_delete = Vec::new();
	let mut remote_paths: Vec<_> = remote_hashes.keys().cloned().collect();
	remote_paths.sort_unstable(); // Sort to avoid iter_over_hash_type issue and ensure determinism
	for remote_path in remote_paths {
		if !local_paths_str.contains(&remote_path) {
			to_delete.push(remote_path);
		}
	}

	SyncActions {
		to_upload,
		to_delete,
	}
}

/// Aborts synchronization when too many local files are considered for sync.
fn ensure_sync_file_limit(file_count: usize, max_files_to_sync: usize) -> Result<()> {
	if file_count > max_files_to_sync {
		bail!(
			"Aborting synchronization: {} files to sync exceeds the limit of {}.\nIf this is expected, increase `sync.sftp.max_files_to_sync` in your configuration.",
			file_count,
			max_files_to_sync
		);
	}

	Ok(())
}

/// SFTP naturally resolves paths not starting with `/` relative to the user's home directory.
/// It does NOT expand `~/` or `$HOME/` like a shell would. Therefore, we strip them so SFTP
/// looks in the home directory instead of looking for literal `~` or `$HOME` folders.
fn resolve_sftp_path(remote_path: &str) -> &str {
	remote_path
		.strip_prefix("~/")
		.or_else(|| remote_path.strip_prefix("$HOME/"))
		.unwrap_or_else(|| {
			if remote_path == "~" || remote_path == "$HOME" {
				"."
			} else {
				remote_path
			}
		})
}

/// Uploads a file to a remote SFTP server using an existing session.
/// We provide our own upload method because `async-ssh2-tokio`'s `upload_file`
/// creates a new channel for every file and does not allow specifying file attributes (like permissions)
/// atomically on creation, leading to race conditions where sensitive files might be readable.
async fn upload_file(
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

	let mut local_file = AsyncFile::open(local_path)
		.await
		.wrap_err_with(|| format!("Failed to open local file: {}", local_path.display()))?;
	let mut local_file_buffered = AsyncBufReader::new(&mut local_file);

	let sftp_path = resolve_sftp_path(remote_path);

	if let Ok(attrs) = sftp.symlink_metadata(sftp_path).await
		&& attrs.is_symlink()
	{
		sftp.remove_file(sftp_path)
			.await
			.wrap_err_with(|| format!("Failed to remove remote symlink: {sftp_path}"))?;
	}

	if matches!(permissions, SftpPermissions::Recreate) {
		let should_remove = sftp
			.metadata(sftp_path)
			.await
			.map(|attrs| {
				attrs
					.permissions
					.map_or_else(|| true, |p| (p & 0o777) != secure_mode)
			})
			.unwrap_or(true); // Default to true if metadata fails
		if should_remove && let Err(e) = sftp.remove_file(sftp_path).await {
			debug!(error = %e, path = sftp_path, "Failed to remove pre-existing file or file did not exist");
		}
	}

	let open_flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
	let mut remote_file = sftp
		.open_with_flags_and_attributes(sftp_path, open_flags, perm_attrs.clone())
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {sftp_path}"))?;

	if matches!(permissions, SftpPermissions::Setstat)
		&& let Err(e) = remote_file.set_metadata(perm_attrs).await
	{
		warn!(
			error = %e,
			path = sftp_path,
			"Failed to enforce file permissions via fsetstat. \
			 Consider setting `sync.sftp.permissions = \"recreate\"` in your config."
		);
	}

	async_copy(&mut local_file_buffered, &mut remote_file)
		.await
		.wrap_err("Failed to write to remote file")?;

	Ok(())
}

/// Target and actions for a synchronization operation.
struct SyncTarget<'a> {
	/// The local project root directory.
	project_root: &'a Path,
	/// The remote directory path.
	remote_dir: &'a str,
	/// The synchronization actions to execute.
	actions: SyncActions,
}

/// Executes the synchronization actions by uploading and deleting files via SFTP.
async fn apply_sync_actions(
	client: &Client,
	config: &Config,
	target: SyncTarget<'_>,
	stats: &mut Stats,
	spinner: Option<&ProgressBar>,
) -> Result<()> {
	let SyncTarget {
		project_root,
		remote_dir,
		actions,
	} = target;

	if actions.to_delete.is_empty() && actions.to_upload.is_empty() {
		return Ok(());
	}

	let channel = client
		.get_channel()
		.await
		.wrap_err("Failed to get SFTP channel")?;
	channel
		.request_subsystem(true, "sftp")
		.await
		.wrap_err("Failed to request SFTP subsystem")?;
	let sftp = SftpSession::new(channel.into_stream())
		.await
		.wrap_err("Failed to initialize SFTP session")?;

	// Remove deleted files via SFTP
	for path in &actions.to_delete {
		let full_path = format!("{remote_dir}/{path}");
		let sftp_path = resolve_sftp_path(&full_path);
		if let Err(e) = sftp.remove_file(sftp_path).await {
			warn!(error = %e, path = sftp_path, "Failed to delete remote file");
		}
		stats.deleted = stats.deleted.saturating_add(1);
	}

	// Pre-create subdirectories respecting umask
	let mut dirs_to_create = HashSet::new();
	for rel_path in &actions.to_upload {
		if let Some(parent) = rel_path.parent() {
			let p_str = parent.to_string_lossy().into_owned();
			if !p_str.is_empty() {
				dirs_to_create.insert(format!("{remote_dir}/{p_str}"));
			}
		}
	}

	if !dirs_to_create.is_empty() {
		let mkdirs = dirs_to_create
			.into_iter()
			.map(|d| shell_quote_path(&d))
			.collect::<Vec<_>>()
			.join(" ");
		let mkdir_cmd = format!("umask {} && mkdir -p -- {mkdirs}", config.ssh.umask);
		client
			.execute(&mkdir_cmd)
			.await
			.wrap_err("Failed to create remote directories")?;
	}

	// Upload files and change permissions to match local user permissions (respecting umask)
	let total_to_upload = actions.to_upload.len();
	for (i, rel_path) in actions.to_upload.into_iter().enumerate() {
		if let Some(s) = spinner {
			s.set_message(format!(
				"Synchronizing files... ({}/{total_to_upload})",
				i.saturating_add(1)
			));
		}

		let local_path = project_root.join(&rel_path);
		let rel_str = rel_path.to_string_lossy().into_owned();
		let remote_path = format!("{remote_dir}/{rel_str}");

		// Read local permissions
		let local_mode = metadata(&local_path)
			.await
			.wrap_err_with(|| format!("Failed to read metadata for {}", local_path.display()))?
			.permissions()
			.mode();
		// Apply configured umask to local permissions
		let secure_mode = local_mode & !config.ssh.umask.as_u32();

		upload_file(
			&sftp,
			&local_path,
			&remote_path,
			secure_mode,
			&config.sync.sftp.permissions,
		)
		.await?;

		stats.uploaded = stats.uploaded.saturating_add(1);
	}

	Ok(())
}

/// Synchronizes a project to a remote server.
#[expect(clippy::module_name_repetitions, reason = "No better name exists")]
pub async fn sync_project(
	config: &Config,
	project_root: &Path,
	options: &Options,
	remote_dir_override: Option<&str>,
	quiet: bool,
) -> Result<Stats> {
	if config.sync.engine != SyncEngine::Sftp {
		bail!("Only SFTP sync engine is currently supported");
	}
	info!(
		project_root = %project_root.display(),
		force = options.force,
		include_patterns = options.include.len(),
		exclude_patterns = options.exclude.len(),
		has_remote_override = remote_dir_override.is_some(),
		"Starting project synchronization"
	);

	check_remote_root(&config.sync.remote_root);

	let unique_project_name = compute_unique_project_name(project_root)?;

	let local_files = {
		let project_root = project_root.to_path_buf();
		let exclude = config.sync.exclude.clone();
		let options = options.clone();
		spawn_blocking(move || collect_local_files(&project_root, &exclude, &options))
			.await
			.wrap_err("Failed to join blocking task")??
	};
	info!(local_files = local_files.len(), "Collected local files");
	ensure_sync_file_limit(local_files.len(), config.sync.sftp.max_files_to_sync)?;

	let client = connect(config, quiet).await?;

	let spinner = if quiet {
		None
	} else {
		Some(create_spinner("Synchronizing files...".to_owned()))
	};

	// Compute remote directory base
	let remote_dir = remote_dir_override.map_or_else(
		|| {
			compute_remote_path(
				&config.sync.remote_root,
				&unique_project_name,
				Path::new(""),
			)
		},
		String::from,
	);

	let remote_hashes = fetch_remote_hashes(&client, config, &remote_dir).await?;
	debug!(
		remote_dir = %remote_dir,
		remote_files = remote_hashes.len(),
		"Fetched remote file hashes"
	);

	let mut stats = Stats::default();
	let actions = calculate_sync_actions(&local_files, &remote_hashes, options);
	stats.unchanged = local_files.len().saturating_sub(actions.to_upload.len());
	info!(
		to_upload = actions.to_upload.len(),
		to_delete = actions.to_delete.len(),
		unchanged = stats.unchanged,
		"Calculated synchronization actions"
	);

	apply_sync_actions(
		&client,
		config,
		SyncTarget {
			project_root,
			remote_dir: &remote_dir,
			actions,
		},
		&mut stats,
		spinner.as_ref(),
	)
	.await?;

	if let Some(s) = spinner {
		s.finish_and_clear();
	}
	info!("Sync completed: {:?}", stats);
	if !quiet {
		eprintln!(
			"{} Sync completed: {} uploaded, {} deleted, {} unchanged",
			style("✓").green().bold(),
			stats.uploaded,
			stats.deleted,
			stats.unchanged
		);
	}

	Ok(stats)
}

/// Parses the output of `find . -type f -exec sha256sum {} +` into a `HashMap` mapping paths to hashes.
/// Validates paths to prevent directory traversal attacks during sync.
fn parse_remote_hashes(output: &str) -> HashMap<String, String> {
	let mut remote_hashes = HashMap::new();
	for line in output.lines() {
		if let Some((hash, raw_path)) = line.split_once("  ") {
			let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
			// Validate that the remote path does not contain directory traversal components
			// to prevent malicious deletion attacks during the sync cleanup phase.
			if path.split('/').any(|comp| comp == "..") {
				warn!(
					"Skipping remote file with invalid path traversal components: {}",
					path
				);
			} else {
				remote_hashes.insert(path.to_owned(), hash.to_owned());
			}
		}
	}
	remote_hashes
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use std::fs;
	use tempfile::tempdir;

	#[test]
	fn collect_local_files_basic() {
		let dir = tempdir().unwrap();
		let file_path = dir.path().join("test.txt");
		fs::write(&file_path, "hello").unwrap();

		let files = collect_local_files(dir.path(), &[], &Options::default()).unwrap();
		assert_eq!(files.len(), 1);
		assert_eq!(files.first().unwrap().path.to_string_lossy(), "test.txt");

		let expected_hash = hex::encode(Sha256::digest(b"hello"));
		assert_eq!(files.first().unwrap().hash, expected_hash);
	}

	#[test]
	fn collect_local_files_respects_gitignore() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
		fs::write(dir.path().join("ignored.txt"), "ignored").unwrap();
		fs::write(dir.path().join("kept.txt"), "kept").unwrap();

		let files = collect_local_files(dir.path(), &[], &Options::default()).unwrap();
		let names: Vec<_> = files
			.iter()
			.map(|f| f.path.to_string_lossy().to_string())
			.collect();
		assert!(names.contains(&".gitignore".to_owned()));
		assert!(!names.contains(&"ignored.txt".to_owned()));
		assert!(names.contains(&"kept.txt".to_owned()));
	}

	#[test]
	fn collect_local_files_includes_hidden() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join(".hidden"), "hidden content").unwrap();
		fs::write(dir.path().join("visible.txt"), "visible content").unwrap();

		let files = collect_local_files(dir.path(), &[], &Options::default()).unwrap();
		let names: Vec<_> = files
			.iter()
			.map(|f| f.path.to_string_lossy().to_string())
			.collect();
		assert!(names.contains(&".hidden".to_owned()));
		assert!(names.contains(&"visible.txt".to_owned()));
	}

	#[test]
	fn parse_remote_hashes_traversal() {
		let output = "hash1  ./valid/path.txt\nhash2  ./../invalid/path.txt\nhash3  valid2.txt";
		let hashes = parse_remote_hashes(output);
		assert_eq!(hashes.len(), 2);
		assert_eq!(hashes.get("valid/path.txt").unwrap(), "hash1");
		assert_eq!(hashes.get("valid2.txt").unwrap(), "hash3");
		assert!(!hashes.contains_key("../invalid/path.txt"));
	}

	#[test]
	fn ensure_sync_file_limit_allows_at_limit() {
		ensure_sync_file_limit(2, 2).unwrap();
	}

	#[test]
	fn ensure_sync_file_limit_rejects_above_limit() {
		let err = ensure_sync_file_limit(2, 1).unwrap_err();
		assert_eq!(
			err.to_string(),
			"Aborting synchronization: 2 files to sync exceeds the limit of 1.\nIf this is expected, increase `sync.sftp.max_files_to_sync` in your configuration."
		);
	}

	#[test]
	fn compute_remote_path_relative_check() {
		let root = Path::new("~/.cache/biwa/projects");
		let proj = "test_proj";
		let rel = Path::new("src/main.rs");
		let remote = compute_remote_path(root, proj, rel);
		assert_eq!(remote, "~/.cache/biwa/projects/test_proj/src/main.rs");
	}

	#[test]
	fn shell_quote_path_tilde() {
		assert_eq!(
			shell_quote_path("~/.cache/biwa/projects"),
			"\"$HOME\"/.cache/biwa/projects"
		);
	}

	#[test]
	fn shell_quote_path_absolute() {
		assert_eq!(shell_quote_path("/home/user/.cache"), "/home/user/.cache");
	}

	#[test]
	fn shell_quote_path_special_chars() {
		assert_eq!(
			shell_quote_path("~/my project/dir"),
			"\"$HOME\"/'my project/dir'"
		);
	}

	#[test]
	fn shell_quote_path_bare_tilde() {
		assert_eq!(shell_quote_path("~"), "\"$HOME\"");
	}

	#[test]
	fn shell_quote_path_home_var() {
		assert_eq!(
			shell_quote_path("$HOME/.cache/biwa/projects"),
			"\"$HOME\"/.cache/biwa/projects"
		);
		assert_eq!(shell_quote_path("$HOME"), "\"$HOME\"");
	}

	#[test]
	fn resolve_sftp_path() {
		assert_eq!(super::resolve_sftp_path("~/foo/bar"), "foo/bar");
		assert_eq!(super::resolve_sftp_path("$HOME/foo/bar"), "foo/bar");
		assert_eq!(super::resolve_sftp_path("~"), ".");
		assert_eq!(super::resolve_sftp_path("$HOME"), ".");
		assert_eq!(super::resolve_sftp_path("/absolute/path"), "/absolute/path");
		assert_eq!(super::resolve_sftp_path("relative/path"), "relative/path");
	}
}
