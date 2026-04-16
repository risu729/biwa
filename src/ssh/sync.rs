use crate::Result;
use crate::config::types::{Config, SftpPermissions, SyncEngine};
use crate::ssh::client::Client;
use crate::ui::create_spinner;
use color_eyre::eyre::{Context as _, ContextCompat as _, bail};
use console::style;
use core::mem::take;
use gethostname::gethostname;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use indicatif::ProgressBar;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use tokio::fs::{File as AsyncFile, metadata};
use tokio::io::{BufReader as AsyncBufReader, copy as async_copy};
use tokio::task::spawn_blocking;
use tracing::{debug, info, warn};

/// Separator emitted by the remote sync-state script before file hash lines.
const REMOTE_FILE_MARKER: &str = "__BIWA_FILE_HASHES__";
/// Conservative upper bound for a batched remote `mkdir -p` command.
const MAX_REMOTE_MKDIR_COMMAND_LEN: usize = 4096;

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

/// The local sync state collected from the project root.
#[derive(Debug, Default)]
struct LocalState {
	/// The local files that should exist on the remote side.
	files: Vec<LocalFile>,
	/// The local directories that should exist on the remote side.
	directories: HashSet<String>,
}

/// The remote sync state collected from the project directory.
#[derive(Debug, Default)]
struct RemoteState {
	/// The remote files and their hashes.
	file_hashes: HashMap<String, String>,
	/// The remote directories that currently exist.
	directories: HashSet<String>,
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

/// Returns whether a path matches a globset.
///
/// For directories, also try the path with a trailing slash so patterns like
/// `foo/**` match the `foo` directory entry itself, not just its descendants.
fn path_matches_globset(globset: &GlobSet, path: &Path, is_dir: bool) -> bool {
	let path = path.to_string_lossy();
	if globset.is_match(path.as_ref()) {
		return true;
	}

	is_dir && globset.is_match(format!("{path}/"))
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

/// Returns whether a local path should participate in synchronization.
fn should_sync_path(
	root: &Path,
	path: &Path,
	is_dir: bool,
	is_symlink: bool,
	exclude_globs: Option<&GlobSet>,
	include_globs: Option<&GlobSet>,
) -> Result<Option<PathBuf>> {
	let relative = path.strip_prefix(root).wrap_err("Failed to strip prefix")?;
	if relative.as_os_str().is_empty() {
		return Ok(None);
	}

	if is_symlink {
		return Ok(None);
	}

	if exclude_globs
		.as_ref()
		.is_some_and(|set| path_matches_globset(set, path, is_dir))
	{
		return Ok(None);
	}

	if include_globs
		.as_ref()
		.is_some_and(|set| !path_matches_globset(set, path, is_dir))
	{
		return Ok(None);
	}

	Ok(Some(relative.to_path_buf()))
}

/// Collects local files and directories from the project root, respecting ignore rules.
fn collect_local_state(
	root: &Path,
	config_exclude: &[String],
	options: &Options,
) -> Result<LocalState> {
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

	let mut state = LocalState::default();
	for entry in builder.build() {
		let entry = entry?;
		let path = entry.path();
		let file_type = fs::symlink_metadata(path)
			.wrap_err_with(|| format!("Failed to read metadata for {}", path.display()))?
			.file_type();
		let is_dir = file_type.is_dir();
		let is_symlink = file_type.is_symlink();
		let Some(relative) = should_sync_path(
			root,
			path,
			is_dir,
			is_symlink,
			exclude_globs.as_ref(),
			include_globs.as_ref(),
		)?
		else {
			continue;
		};

		if !is_dir && !is_symlink && path.is_file() {
			state.files.push(LocalFile {
				path: relative,
				hash: hash_file(path)?,
			});
			continue;
		}

		if is_dir {
			state
				.directories
				.insert(relative.to_string_lossy().into_owned());
		}
	}

	Ok(state)
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

/// Extends a directory set with parent directories implied by local files.
fn collect_parent_directories_into(files: &[LocalFile], directories: &mut HashSet<String>) {
	for local_file in files {
		for ancestor in local_file.path.ancestors() {
			if ancestor.as_os_str().is_empty() || ancestor == local_file.path.as_path() {
				continue;
			}
			directories.insert(ancestor.to_string_lossy().into_owned());
		}
	}
}

/// Fetches the current remote directory and file state.
async fn fetch_remote_state(
	client: &Client,
	config: &Config,
	remote_dir: &str,
) -> Result<RemoteState> {
	let quoted_remote_dir = shell_quote_path(remote_dir);
	let dir_mode = format!("{:04o}", 0o777 & !config.ssh.umask.as_u32());
	let quoted_marker = shell_words::quote(REMOTE_FILE_MARKER).into_owned();

	// Create the remote dir, normalize directory permissions, then print directories and file hashes.
	let script = format!(
		"umask {} && mkdir -p -- {quoted_remote_dir} && \
		 if [ -L {quoted_remote_dir} ]; then echo 'Error: remote directory is a symlink' >&2; exit 1; fi && \
		 cd -- {quoted_remote_dir} && \
		 (find . -type d -exec chmod {dir_mode} {{}} + || true) && \
		 (find . -mindepth 1 -type d -print || true) && \
		 printf '%s\n' {quoted_marker} && \
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

	Ok(parse_remote_state(&output))
}

/// Actions to perform during synchronization.
struct SyncActions {
	/// Files to upload to the remote server.
	uploads: Vec<PathBuf>,
	/// Files to delete from the remote server.
	file_deletions: Vec<String>,
	/// Directories to create on the remote server.
	directory_creations: Vec<String>,
	/// Directories to delete from the remote server.
	directory_deletions: Vec<String>,
}

/// Compares local and remote sync state to determine which actions are required.
fn calculate_sync_actions(
	local_state: &LocalState,
	remote_state: &RemoteState,
	options: &Options,
) -> SyncActions {
	let mut desired_dirs = local_state.directories.clone();
	collect_parent_directories_into(&local_state.files, &mut desired_dirs);

	let mut to_upload = Vec::new();
	let mut local_paths_str = HashSet::new();

	for local_file in &local_state.files {
		let rel_path_str = local_file.path.to_string_lossy().into_owned();
		local_paths_str.insert(rel_path_str.clone());

		if !options.force
			&& !remote_state.directories.contains(&rel_path_str)
			&& let Some(remote_hash) = remote_state.file_hashes.get(&rel_path_str)
			&& remote_hash == &local_file.hash
		{
			continue;
		}
		to_upload.push(local_file.path.clone());
	}

	let mut to_delete_files = HashSet::new();
	let mut remote_paths: Vec<_> = remote_state.file_hashes.keys().cloned().collect();
	remote_paths.sort_unstable(); // Sort to avoid iter_over_hash_type issue and ensure determinism
	for remote_path in remote_paths {
		if !local_paths_str.contains(&remote_path) || desired_dirs.contains(&remote_path) {
			to_delete_files.insert(remote_path);
		}
	}

	let mut to_create_dirs = desired_dirs
		.iter()
		.filter(|path| !remote_state.directories.contains(*path))
		.cloned()
		.collect::<Vec<_>>();
	to_create_dirs.sort_unstable();

	let mut to_delete_dirs = remote_state
		.directories
		.iter()
		.filter(|path| !desired_dirs.contains(*path) || local_paths_str.contains(*path))
		.cloned()
		.collect::<Vec<_>>();
	to_delete_dirs.sort_unstable_by(|left, right| {
		let left_depth = left.bytes().filter(|byte| *byte == b'/').count();
		let right_depth = right.bytes().filter(|byte| *byte == b'/').count();
		right_depth.cmp(&left_depth).then_with(|| left.cmp(right))
	});

	let mut to_delete_files = to_delete_files.into_iter().collect::<Vec<_>>();
	to_delete_files.sort_unstable();
	to_upload.sort_unstable();

	SyncActions {
		uploads: to_upload,
		file_deletions: to_delete_files,
		directory_creations: to_create_dirs,
		directory_deletions: to_delete_dirs,
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

/// Returns whether `candidate` is nested beneath `ancestor`.
fn is_nested_directory(candidate: &str, ancestor: &str) -> bool {
	candidate != ancestor && Path::new(candidate).starts_with(ancestor)
}

/// Reduces a directory set to its deepest leaf paths.
fn collect_leaf_directories(paths: &[String]) -> Vec<String> {
	let mut sorted = paths.to_vec();
	sorted.sort_unstable_by(|left, right| {
		let left_depth = left.bytes().filter(|byte| *byte == b'/').count();
		let right_depth = right.bytes().filter(|byte| *byte == b'/').count();
		right_depth.cmp(&left_depth).then_with(|| left.cmp(right))
	});

	let mut leaves = Vec::new();
	for path in sorted {
		if leaves
			.iter()
			.any(|existing: &String| is_nested_directory(existing, &path))
		{
			continue;
		}
		leaves.push(path);
	}

	leaves.sort_unstable();
	leaves
}

/// Deletes remote directories one-by-one over the existing SFTP session.
async fn delete_remote_directories(
	sftp: &SftpSession,
	remote_dir: &str,
	relative_paths: &[String],
) -> usize {
	let mut deleted = 0_usize;
	for path in relative_paths {
		let full_path = format!("{remote_dir}/{path}");
		let sftp_path = resolve_sftp_path(&full_path);
		match sftp.remove_dir(sftp_path).await {
			Ok(()) => {
				deleted = deleted.saturating_add(1);
			}
			Err(error) => {
				warn!(error = %error, path = sftp_path, "Failed to delete remote directory");
			}
		}
	}

	deleted
}

/// Collects the set of directories that must exist before uploading files.
fn collect_directories_to_create(actions: &SyncActions) -> Vec<String> {
	let mut directories = actions
		.directory_creations
		.iter()
		.cloned()
		.collect::<HashSet<_>>();
	for rel_path in &actions.uploads {
		if let Some(parent) = rel_path.parent() {
			let parent = parent.to_string_lossy().into_owned();
			if !parent.is_empty() {
				directories.insert(parent);
			}
		}
	}

	let mut directories = directories.into_iter().collect::<Vec<_>>();
	directories.sort_unstable();
	collect_leaf_directories(&directories)
}

/// Splits directory creation into bounded `mkdir -p` batches.
fn build_mkdir_commands(umask: &str, remote_dir: &str, relative_paths: &[String]) -> Vec<String> {
	if relative_paths.is_empty() {
		return Vec::new();
	}

	let prefix = format!("umask {umask} && mkdir -p --");
	let mut commands = Vec::new();
	let mut current = prefix.clone();

	for quoted_path in relative_paths
		.iter()
		.map(|path| format!("{remote_dir}/{path}"))
		.map(|path| shell_quote_path(&path))
	{
		let projected_len = current
			.len()
			.saturating_add(1)
			.saturating_add(quoted_path.len());
		if current.len() > prefix.len() && projected_len > MAX_REMOTE_MKDIR_COMMAND_LEN {
			commands.push(take(&mut current));
			current.clone_from(&prefix);
		}

		current.push(' ');
		current.push_str(&quoted_path);
	}

	if current.len() > prefix.len() {
		commands.push(current);
	}

	commands
}

/// Creates remote directories with the configured umask.
async fn create_remote_directories(
	client: &Client,
	config: &Config,
	remote_dir: &str,
	relative_paths: &[String],
) -> Result<()> {
	for mkdir_cmd in build_mkdir_commands(&config.ssh.umask.to_string(), remote_dir, relative_paths)
	{
		let result = client
			.execute(&mkdir_cmd)
			.await
			.wrap_err("Failed to create remote directories")?;
		if result.exit_status != 0 {
			bail!(
				"Failed to create remote directories: {}",
				result.stderr.trim()
			);
		}
	}

	Ok(())
}

/// Uploads a file to a remote SFTP server using an existing session.
///
/// We provide our own upload method so we can set file attributes atomically on
/// creation (`open_with_flags_and_attributes`), avoiding races where sensitive
/// files might be briefly world-readable.
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
			.map_or(true, |attrs| {
				attrs
					.permissions
					.map_or_else(|| true, |p| (p & 0o777) != secure_mode)
			}); // Default to true if metadata fails
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

	if actions.file_deletions.is_empty()
		&& actions.directory_creations.is_empty()
		&& actions.directory_deletions.is_empty()
		&& actions.uploads.is_empty()
	{
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

	// Remove deleted files via SFTP.
	for path in &actions.file_deletions {
		let full_path = format!("{remote_dir}/{path}");
		let sftp_path = resolve_sftp_path(&full_path);
		if let Err(e) = sftp.remove_file(sftp_path).await {
			warn!(error = %e, path = sftp_path, "Failed to delete remote file");
		} else {
			stats.deleted = stats.deleted.saturating_add(1);
		}
	}

	// Remove deleted directories deepest-first so parents become empty first.
	stats.deleted = stats.deleted.saturating_add(
		delete_remote_directories(&sftp, remote_dir, &actions.directory_deletions).await,
	);

	// Pre-create directories respecting umask.
	let dirs_to_create = collect_directories_to_create(&actions);
	create_remote_directories(client, config, remote_dir, &dirs_to_create).await?;

	// Upload files and change permissions to match local user permissions (respecting umask)
	let total_to_upload = actions.uploads.len();
	for (i, rel_path) in actions.uploads.into_iter().enumerate() {
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
	client: &Client,
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

	let unique_project_name = compute_unique_project_name(project_root)?;

	let local_state = {
		let project_root = project_root.to_path_buf();
		let exclude = config.sync.exclude.clone();
		let options = options.clone();
		spawn_blocking(move || collect_local_state(&project_root, &exclude, &options))
			.await
			.wrap_err("Failed to join blocking task")??
	};
	info!(
		local_directories = local_state.directories.len(),
		local_files = local_state.files.len(),
		"Collected local sync state"
	);
	ensure_sync_file_limit(local_state.files.len(), config.sync.sftp.max_files_to_sync)?;

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

	let remote_state = fetch_remote_state(client, config, &remote_dir).await?;
	debug!(
		remote_dir = %remote_dir,
		remote_directories = remote_state.directories.len(),
		remote_files = remote_state.file_hashes.len(),
		"Fetched remote sync state"
	);

	let mut stats = Stats::default();
	let actions = calculate_sync_actions(&local_state, &remote_state, options);
	stats.unchanged = local_state
		.files
		.len()
		.saturating_sub(actions.uploads.len());
	info!(
		to_create_dirs = actions.directory_creations.len(),
		to_delete_dirs = actions.directory_deletions.len(),
		to_upload = actions.uploads.len(),
		to_delete = actions.file_deletions.len(),
		unchanged = stats.unchanged,
		"Calculated synchronization actions"
	);

	apply_sync_actions(
		client,
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

/// Normalizes a remote relative path and rejects absolute or traversal paths.
fn parse_remote_path(raw_path: &str, entry_kind: &str) -> Option<String> {
	let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
	if path.is_empty() || path == "." {
		return None;
	}

	if path.starts_with('/')
		|| path == "~"
		|| path.starts_with("~/")
		|| path == "$HOME"
		|| path.starts_with("$HOME/")
	{
		warn!(
			"Skipping remote {entry_kind} with invalid absolute path: {}",
			path
		);
		return None;
	}

	if path.split('/').any(|comp| comp == "..") {
		warn!(
			"Skipping remote {entry_kind} with invalid path traversal components: {}",
			path
		);
		return None;
	}

	Some(path.to_owned())
}

/// Parses the output of the remote sync-state script into a directory set and file hash map.
fn parse_remote_state(output: &str) -> RemoteState {
	let mut remote_state = RemoteState::default();
	let mut parsing_files = false;

	for line in output.lines() {
		if line == REMOTE_FILE_MARKER {
			parsing_files = true;
			continue;
		}

		if parsing_files {
			if let Some((hash, raw_path)) = line.split_once("  ")
				&& let Some(path) = parse_remote_path(raw_path, "file")
			{
				remote_state.file_hashes.insert(path, hash.to_owned());
			}
			continue;
		}

		if let Some(path) = parse_remote_path(line, "directory") {
			remote_state.directories.insert(path);
		}
	}

	remote_state
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

		let files = collect_local_state(dir.path(), &[], &Options::default())
			.unwrap()
			.files;
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

		let files = collect_local_state(dir.path(), &[], &Options::default())
			.unwrap()
			.files;
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

		let files = collect_local_state(dir.path(), &[], &Options::default())
			.unwrap()
			.files;
		let names: Vec<_> = files
			.iter()
			.map(|f| f.path.to_string_lossy().to_string())
			.collect();
		assert!(names.contains(&".hidden".to_owned()));
		assert!(names.contains(&"visible.txt".to_owned()));
	}

	#[test]
	fn collect_local_state_includes_empty_directories() {
		let dir = tempdir().unwrap();
		fs::create_dir_all(dir.path().join("empty/nested")).unwrap();
		fs::write(dir.path().join("file.txt"), "hello").unwrap();

		let state = collect_local_state(dir.path(), &[], &Options::default()).unwrap();
		assert!(state.directories.contains("empty"));
		assert!(state.directories.contains("empty/nested"));
		assert_eq!(state.files.len(), 1);
	}

	#[test]
	fn collect_local_state_respects_include_for_empty_directories() {
		let dir = tempdir().unwrap();
		fs::create_dir_all(dir.path().join("kept")).unwrap();
		fs::create_dir_all(dir.path().join("ignored")).unwrap();

		let state = collect_local_state(
			dir.path(),
			&[],
			&Options {
				include: vec![dir.path().join("kept").to_string_lossy().into_owned()],
				..Options::default()
			},
		)
		.unwrap();

		assert!(state.directories.contains("kept"));
		assert!(!state.directories.contains("ignored"));
	}

	#[test]
	fn collect_local_state_respects_glob_exclude_for_empty_directories() {
		let dir = tempdir().unwrap();
		fs::create_dir_all(dir.path().join("tests")).unwrap();
		fs::create_dir_all(dir.path().join("kept")).unwrap();

		let state = collect_local_state(
			dir.path(),
			&[],
			&Options {
				exclude: vec![
					dir.path()
						.join("tests")
						.join("**")
						.to_string_lossy()
						.into_owned(),
				],
				..Options::default()
			},
		)
		.unwrap();

		assert!(!state.directories.contains("tests"));
		assert!(state.directories.contains("kept"));
	}

	#[cfg(unix)]
	#[test]
	fn collect_local_state_skips_symlink_entries() {
		use std::os::unix::fs::symlink;

		let dir = tempdir().unwrap();
		fs::create_dir_all(dir.path().join("real-dir")).unwrap();
		fs::write(dir.path().join("real-file.txt"), "hello").unwrap();
		symlink(dir.path().join("real-dir"), dir.path().join("dir-link")).unwrap();
		symlink(
			dir.path().join("real-file.txt"),
			dir.path().join("file-link.txt"),
		)
		.unwrap();

		let state = collect_local_state(dir.path(), &[], &Options::default()).unwrap();
		let file_paths = state
			.files
			.iter()
			.map(|file| file.path.to_string_lossy().into_owned())
			.collect::<Vec<_>>();

		assert!(state.directories.contains("real-dir"));
		assert!(!state.directories.contains("dir-link"));
		assert!(file_paths.contains(&"real-file.txt".to_owned()));
		assert!(!file_paths.contains(&"file-link.txt".to_owned()));
	}

	#[test]
	fn parse_remote_hashes_traversal() {
		let output = "__BIWA_FILE_HASHES__\nhash1  ./valid/path.txt\nhash2  ./../invalid/path.txt\nhash3  valid2.txt";
		let hashes = parse_remote_state(output).file_hashes;
		assert_eq!(hashes.len(), 2);
		assert_eq!(hashes.get("valid/path.txt").unwrap(), "hash1");
		assert_eq!(hashes.get("valid2.txt").unwrap(), "hash3");
		assert!(!hashes.contains_key("../invalid/path.txt"));
	}

	#[test]
	fn parse_remote_state_rejects_absolute_paths() {
		let output = "/etc\n~/dotdir\n$HOME/secret\n__BIWA_FILE_HASHES__\nhash1  /etc/passwd\nhash2  ./valid.txt";
		let state = parse_remote_state(output);
		assert!(state.directories.is_empty());
		assert_eq!(state.file_hashes.len(), 1);
		assert_eq!(state.file_hashes.get("valid.txt").unwrap(), "hash2");
	}

	#[test]
	fn parse_remote_state_collects_directories() {
		let output = "./empty\n./nested/child\n__BIWA_FILE_HASHES__\nhash1  ./nested/file.txt";
		let state = parse_remote_state(output);
		assert!(state.directories.contains("empty"));
		assert!(state.directories.contains("nested/child"));
		assert_eq!(state.file_hashes.get("nested/file.txt").unwrap(), "hash1");
	}

	#[test]
	fn calculate_sync_actions_creates_and_deletes_empty_directories() {
		let actions = calculate_sync_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::from(["empty".to_owned()]),
			},
			&RemoteState {
				file_hashes: HashMap::new(),
				directories: HashSet::from(["stale".to_owned()]),
			},
			&Options::default(),
		);

		assert_eq!(actions.directory_creations, vec!["empty".to_owned()]);
		assert_eq!(actions.directory_deletions, vec!["stale".to_owned()]);
		assert!(actions.file_deletions.is_empty());
		assert!(actions.uploads.is_empty());
	}

	#[test]
	fn calculate_sync_actions_preserves_directory_when_last_file_removed() {
		let actions = calculate_sync_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::from(["dir".to_owned()]),
			},
			&RemoteState {
				file_hashes: HashMap::from([("dir/file.txt".to_owned(), "hash".to_owned())]),
				directories: HashSet::from(["dir".to_owned()]),
			},
			&Options::default(),
		);

		assert_eq!(actions.file_deletions, vec!["dir/file.txt".to_owned()]);
		assert!(actions.directory_deletions.is_empty());
		assert!(actions.directory_creations.is_empty());
	}

	#[test]
	fn calculate_sync_actions_deletes_directories_deepest_first() {
		let actions = calculate_sync_actions(
			&LocalState::default(),
			&RemoteState {
				file_hashes: HashMap::new(),
				directories: HashSet::from(["a".to_owned(), "a/b".to_owned(), "a/b/c".to_owned()]),
			},
			&Options::default(),
		);

		assert_eq!(
			actions.directory_deletions,
			vec!["a/b/c".to_owned(), "a/b".to_owned(), "a".to_owned()]
		);
	}

	#[test]
	fn collect_leaf_directories_prefers_deepest_paths() {
		let leaves = collect_leaf_directories(&[
			"a".to_owned(),
			"a/b".to_owned(),
			"a/b/c".to_owned(),
			"a/d".to_owned(),
			"e".to_owned(),
		]);

		assert_eq!(
			leaves,
			vec!["a/b/c".to_owned(), "a/d".to_owned(), "e".to_owned()]
		);
	}

	#[test]
	fn build_mkdir_commands_chunks_large_directory_sets() {
		let relative_paths = (0..300)
			.map(|i| format!("very-long-directory-name-{i:03}/nested"))
			.collect::<Vec<_>>();
		let commands = build_mkdir_commands("0077", "~/.cache/biwa/projects/demo", &relative_paths);

		assert!(commands.len() > 1);
		assert!(
			commands
				.iter()
				.all(|command| command.starts_with("umask 0077 && mkdir -p -- "))
		);
		assert!(
			commands
				.iter()
				.all(|command| command.len() <= MAX_REMOTE_MKDIR_COMMAND_LEN)
		);
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
