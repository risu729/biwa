#[cfg(test)]
use super::sync_paths::{MAX_REMOTE_MKDIR_COMMAND_LEN, compute_remote_path};
use super::sync_paths::{build_mkdir_commands, collect_leaf_directories, resolve_sftp_path};
use crate::Result;
use crate::config::types::{Config, SftpPermissions, SyncEngine};
use crate::ssh::client::Client;
use crate::ui::create_spinner;
use color_eyre::eyre::{Context as _, ContextCompat as _, bail};
use console::style;
use core::result::Result as CoreResult;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::Match;
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use indicatif::ProgressBar;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::process;
use tokio::fs::{self as async_fs, File as AsyncFile, OpenOptions as AsyncOpenOptions, metadata};
use tokio::io::{
	AsyncReadExt as _, AsyncWriteExt as _, BufReader as AsyncBufReader, copy as async_copy,
};
use tokio::task::spawn_blocking;
use tracing::{debug, info, warn};

/// Separator emitted by the remote sync-state script before file hash lines.
const REMOTE_FILE_MARKER: &str = "__BIWA_FILE_HASHES__";
/// Separator emitted by the remote sync-state script before symlink lines.
const REMOTE_SYMLINK_MARKER: &str = "__BIWA_SYMLINKS__";
/// Separator emitted by the remote sync-state script before directory paths.
const REMOTE_DIRECTORY_MARKER: &str = "__BIWA_DIRECTORIES__";

/// Computes the remote directory path for a given project.
///
/// This is the directory where synced files are stored on the remote server.
pub fn compute_project_remote_dir(config: &Config, project_root: &Path) -> Result<String> {
	super::sync_paths::compute_project_remote_dir(config, project_root)
}

/// Returns the 8-character hex hash of the local machine hostname.
#[must_use]
pub fn compute_client_host_hash() -> String {
	super::sync_paths::compute_client_host_hash()
}

/// Returns whether a remote directory matches biwa's default project directory layout.
#[must_use]
pub fn is_default_biwa_remote_dir(remote_dir: &str, remote_root: &Path, host_hash: &str) -> bool {
	super::sync_paths::is_default_biwa_remote_dir(remote_dir, remote_root, host_hash)
}

/// Returns whether a remote directory matches any known biwa project directory layout.
#[must_use]
pub fn is_biwa_remote_dir(remote_dir: &str, remote_root: &Path) -> bool {
	super::sync_paths::is_biwa_remote_dir(remote_dir, remote_root)
}

/// Shell-quotes a remote path while preserving home directory expansion.
pub(super) fn shell_quote_path(path: &str) -> String {
	super::sync_paths::shell_quote_path(path)
}

/// Statistics for a synchronization operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
	/// Number of files uploaded.
	pub uploaded: usize,
	/// Number of files downloaded.
	pub downloaded: usize,
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
	/// Local symlinks in the selected synchronization scope.
	symlinks: HashSet<String>,
}

/// The remote sync state collected from the project directory.
#[derive(Debug, Default)]
struct RemoteState {
	/// The remote files and their hashes.
	file_hashes: HashMap<String, String>,
	/// The remote directories that currently exist.
	directories: HashSet<String>,
	/// The remote symlinks that currently exist.
	symlinks: HashSet<String>,
}

/// Direction for a synchronization operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
	/// Push local project contents to the remote directory.
	Push,
	/// Pull remote project contents into the local sync root.
	Pull,
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

/// Builds the combined config and CLI exclude glob set plus the CLI include glob set.
fn build_sync_globsets(
	config_exclude: &[String],
	options: &Options,
) -> Result<(Option<GlobSet>, Option<GlobSet>)> {
	let combined_exclude = config_exclude
		.iter()
		.chain(options.exclude.iter())
		.map(ToString::to_string)
		.collect::<Vec<_>>();
	Ok((
		build_globset(&combined_exclude)?,
		build_globset(&options.include)?,
	))
}

/// A gitignore matcher rooted at the directory containing its ignore file.
struct PullIgnore {
	/// Directory that relative ignore patterns are resolved against.
	root: PathBuf,
	/// Compiled gitignore-style matcher.
	matcher: Gitignore,
}

/// Ignore rules used when deciding which remote paths are allowed to pull locally.
#[derive(Default)]
struct PullIgnoreMatcher {
	/// Matchers ordered from lower to higher precedence.
	matchers: Vec<PullIgnore>,
}

impl PullIgnoreMatcher {
	/// Returns whether the local path equivalent is ignored.
	fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
		let mut ignored = false;
		for matcher in &self.matchers {
			if !path.starts_with(&matcher.root) {
				continue;
			}

			match matcher.matcher.matched_path_or_any_parents(path, is_dir) {
				Match::None => {}
				Match::Ignore(_) => ignored = true,
				Match::Whitelist(_) => ignored = false,
			}
		}
		ignored
	}
}

/// Returns the ignore roots that can affect paths under the sync root.
fn pull_ignore_roots(root: &Path) -> Vec<PathBuf> {
	let Some(git_root) = root.ancestors().find(|path| path.join(".git").exists()) else {
		return vec![root.to_path_buf()];
	};
	let mut roots = root
		.ancestors()
		.take_while(|path| path.starts_with(git_root))
		.map(Path::to_path_buf)
		.collect::<Vec<_>>();
	roots.reverse();
	roots
}

/// Adds one ignore file if it exists.
fn add_pull_ignore_file(
	matchers: &mut Vec<PullIgnore>,
	root: &Path,
	ignore_file: &Path,
) -> Result<()> {
	if !ignore_file.is_file() {
		return Ok(());
	}

	let mut builder = GitignoreBuilder::new(root);
	if let Some(error) = builder.add(ignore_file) {
		bail!(
			"Failed to parse ignore file {}: {error}",
			ignore_file.display()
		);
	}
	let matcher = builder
		.build()
		.wrap_err_with(|| format!("Failed to build ignore matcher: {}", ignore_file.display()))?;
	matchers.push(PullIgnore {
		root: root.to_path_buf(),
		matcher,
	});
	Ok(())
}

/// Adds ignore files found below the sync root in deterministic directory order.
fn add_descendant_pull_ignore_files(matchers: &mut Vec<PullIgnore>, root: &Path) -> Result<()> {
	let mut builder = WalkBuilder::new(root);
	builder.standard_filters(true);
	builder.add_custom_ignore_filename(".biwaignore");
	builder.hidden(false);
	builder.require_git(false);

	let mut ignore_roots = HashSet::new();
	for entry in builder.build() {
		let entry = entry?;
		let path = entry.path();
		if path == root {
			continue;
		}
		let file_name = entry.file_name();
		if (file_name == ".gitignore" || file_name == ".ignore" || file_name == ".biwaignore")
			&& let Some(parent) = path.parent()
			&& parent != root
		{
			ignore_roots.insert(parent.to_path_buf());
		}
	}

	let mut ignore_roots = ignore_roots.into_iter().collect::<Vec<_>>();
	ignore_roots.sort_unstable();
	for ignore_root in ignore_roots {
		add_pull_ignore_file(matchers, &ignore_root, &ignore_root.join(".gitignore"))?;
		add_pull_ignore_file(matchers, &ignore_root, &ignore_root.join(".ignore"))?;
		add_pull_ignore_file(matchers, &ignore_root, &ignore_root.join(".biwaignore"))?;
	}

	Ok(())
}

/// Builds gitignore-style rules that remote pull filtering must respect.
fn build_pull_ignore_matcher(root: &Path) -> Result<PullIgnoreMatcher> {
	let roots = pull_ignore_roots(root);
	let mut matchers = Vec::new();

	if let Some(git_root) = roots.first() {
		add_pull_ignore_file(
			&mut matchers,
			git_root,
			&git_root.join(".git").join("info").join("exclude"),
		)?;
	}

	for ignore_root in roots {
		add_pull_ignore_file(&mut matchers, &ignore_root, &ignore_root.join(".gitignore"))?;
		add_pull_ignore_file(&mut matchers, &ignore_root, &ignore_root.join(".ignore"))?;
		add_pull_ignore_file(
			&mut matchers,
			&ignore_root,
			&ignore_root.join(".biwaignore"),
		)?;
	}
	add_descendant_pull_ignore_files(&mut matchers, root)?;

	Ok(PullIgnoreMatcher { matchers })
}

/// Returns a checked relative path for local pull operations.
fn checked_relative_path(relative_path: &str) -> Result<PathBuf> {
	let path = Path::new(relative_path);
	let mut checked = PathBuf::new();

	for component in path.components() {
		match component {
			Component::Normal(component) => checked.push(component),
			Component::CurDir => {}
			Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
				bail!("Refusing to synchronize unsafe path: {relative_path}");
			}
		}
	}

	if checked.as_os_str().is_empty() {
		bail!("Refusing to synchronize empty relative path");
	}

	Ok(checked)
}

/// Resolves a checked relative path under the local sync root.
fn checked_local_path(root: &Path, relative_path: &str) -> Result<PathBuf> {
	Ok(root.join(checked_relative_path(relative_path)?))
}

/// Returns whether a remote relative path should participate in pull synchronization.
fn should_sync_remote_path(
	root: &Path,
	relative_path: &str,
	is_dir: bool,
	ignore_matcher: &PullIgnoreMatcher,
	exclude_globs: Option<&GlobSet>,
	include_globs: Option<&GlobSet>,
) -> Result<bool> {
	let local_equivalent = root.join(checked_relative_path(relative_path)?);

	if ignore_matcher.is_ignored(&local_equivalent, is_dir) {
		return Ok(false);
	}

	if exclude_globs
		.as_ref()
		.is_some_and(|set| path_matches_globset(set, &local_equivalent, is_dir))
	{
		return Ok(false);
	}

	if include_globs
		.as_ref()
		.is_some_and(|set| !path_matches_globset(set, &local_equivalent, is_dir))
	{
		return Ok(false);
	}

	Ok(true)
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
	exclude_globs: Option<&GlobSet>,
	include_globs: Option<&GlobSet>,
) -> Result<Option<PathBuf>> {
	let relative = path.strip_prefix(root).wrap_err("Failed to strip prefix")?;
	if relative.as_os_str().is_empty() {
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

	let (exclude_globs, include_globs) = build_sync_globsets(config_exclude, options)?;

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
			exclude_globs.as_ref(),
			include_globs.as_ref(),
		)?
		else {
			continue;
		};
		if is_symlink {
			state
				.symlinks
				.insert(relative.to_string_lossy().into_owned());
			continue;
		}

		if !is_dir && path.is_file() {
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

/// Extends a directory set with parent directories implied by file paths.
fn collect_parent_directories_into<'a>(
	paths: impl IntoIterator<Item = &'a Path>,
	directories: &mut HashSet<String>,
) {
	for path in paths {
		for ancestor in path.ancestors() {
			if ancestor.as_os_str().is_empty() || ancestor == path {
				continue;
			}
			directories.insert(ancestor.to_string_lossy().into_owned());
		}
	}
}

/// Builds the remote shell script that emits directory, symlink, and file hash state.
fn build_remote_state_script(config: &Config, remote_dir: &str, create_remote_dir: bool) -> String {
	let quoted_remote_dir = shell_quote_path(remote_dir);
	let quoted_marker = shell_words::quote(REMOTE_FILE_MARKER).into_owned();
	let quoted_symlink_marker = shell_words::quote(REMOTE_SYMLINK_MARKER).into_owned();
	let quoted_directory_marker = shell_words::quote(REMOTE_DIRECTORY_MARKER).into_owned();
	let prepare_remote_dir = if create_remote_dir {
		format!("mkdir -p -- {quoted_remote_dir} &&")
	} else {
		format!(
			"if [ ! -e {quoted_remote_dir} ]; then echo 'Error: remote directory does not exist' >&2; exit 1; fi &&"
		)
	};
	let normalize_remote_dirs = if create_remote_dir {
		let dir_mode = format!("{:04o}", 0o777 & !config.ssh.umask.as_u32());
		format!("(find . -type d -exec chmod {dir_mode} {{}} + || true) &&")
	} else {
		String::new()
	};

	// Prepare the remote dir, normalize owned push directories, then print
	// directories, symlinks, and file hashes without following symlinks.
	format!(
		"umask {} && {prepare_remote_dir} \
		 if [ -L {quoted_remote_dir} ]; then echo 'Error: remote directory is a symlink' >&2; exit 1; fi && \
		 if [ ! -d {quoted_remote_dir} ]; then echo 'Error: remote directory is not a directory' >&2; exit 1; fi && \
		 cd -- {quoted_remote_dir} && \
		 {normalize_remote_dirs} \
		 printf '%s\\0' {quoted_directory_marker} && \
		 find . -mindepth 1 -type d -print0 && \
		 printf '%s\\0' {quoted_symlink_marker} && \
		 find . -type l -print0 && \
		 printf '%s\\0' {quoted_marker} && \
		 find . -type f -exec sha256sum -z {{}} +",
		config.ssh.umask
	)
}

/// Fetches the current remote directory and file state.
async fn fetch_remote_state(
	client: &Client,
	config: &Config,
	remote_dir: &str,
	create_remote_dir: bool,
) -> Result<RemoteState> {
	let script = build_remote_state_script(config, remote_dir, create_remote_dir);

	let result = client
		.execute(&script)
		.await
		.wrap_err("Failed to fetch remote state")?;

	if result.exit_status != 0 {
		let stderr = result.stderr.trim();
		if stderr.contains("remote directory is a symlink") {
			bail!("remote directory is a symlink");
		}
		if stderr.contains("remote directory does not exist") {
			bail!("remote directory does not exist");
		}
		if stderr.contains("remote directory is not a directory") {
			bail!("remote directory is not a directory");
		}
		bail!(
			"Remote script failed with code {}: {}",
			result.exit_status,
			stderr
		);
	}

	let output = result.stdout;

	parse_remote_state(&output)
}

/// Actions to perform during synchronization.
struct PushActions {
	/// Files to upload to the remote server.
	uploads: Vec<PathBuf>,
	/// Files to delete from the remote server.
	file_deletions: Vec<String>,
	/// Directories to create on the remote server.
	directory_creations: Vec<String>,
	/// Directories to delete from the remote server.
	directory_deletions: Vec<String>,
}

/// Actions to perform during pull synchronization.
struct PullActions {
	/// Files to download from the remote server.
	downloads: Vec<Download>,
	/// Selected remote files that already match locally.
	unchanged: usize,
	/// Files to delete from the local sync root.
	file_deletions: Vec<String>,
	/// Directories to create under the local sync root.
	directory_creations: Vec<String>,
	/// Directories to delete from the local sync root.
	directory_deletions: Vec<String>,
}

/// A remote file that must be staged for a pull operation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Download {
	/// Checked path relative to both synchronization roots.
	path: String,
	/// SHA-256 digest captured by the remote inventory.
	expected_hash: String,
}

/// Compares local and remote sync state to determine which actions are required.
fn calculate_push_actions(
	local_state: &LocalState,
	remote_state: &RemoteState,
	options: &Options,
) -> PushActions {
	let mut desired_dirs = local_state.directories.clone();
	collect_parent_directories_into(
		local_state.files.iter().map(|file| file.path.as_path()),
		&mut desired_dirs,
	);

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
	sort_paths_deepest_first(&mut to_delete_dirs);

	let mut to_delete_files = to_delete_files.into_iter().collect::<Vec<_>>();
	to_delete_files.sort_unstable();
	to_upload.sort_unstable();

	PushActions {
		uploads: to_upload,
		file_deletions: to_delete_files,
		directory_creations: to_create_dirs,
		directory_deletions: to_delete_dirs,
	}
}

/// Compares local and remote sync state to determine pull actions.
fn calculate_pull_actions(
	local_state: &LocalState,
	remote_state: &RemoteState,
	options: &Options,
) -> PullActions {
	let mut desired_dirs = remote_state.directories.clone();
	collect_parent_directories_into(
		remote_state.file_hashes.keys().map(Path::new),
		&mut desired_dirs,
	);

	let local_files = local_state
		.files
		.iter()
		.map(|file| (file.path.to_string_lossy().into_owned(), file.hash.as_str()))
		.collect::<HashMap<_, _>>();
	let remote_file_set = remote_state
		.file_hashes
		.keys()
		.cloned()
		.collect::<HashSet<_>>();

	let mut downloads = Vec::new();
	let mut unchanged = 0_usize;
	let mut remote_paths = remote_state
		.file_hashes
		.iter()
		.collect::<Vec<(&String, &String)>>();
	remote_paths.sort_unstable_by_key(|(path, _)| *path);
	for (remote_path, remote_hash) in remote_paths {
		if should_download_remote_file(
			local_files.get(remote_path).copied(),
			remote_hash,
			options.force,
		) {
			downloads.push(Download {
				path: remote_path.clone(),
				expected_hash: remote_hash.clone(),
			});
		} else {
			unchanged = unchanged.saturating_add(1);
		}
	}

	let mut file_deletions = local_files
		.keys()
		.filter(|path| !remote_file_set.contains(*path) || desired_dirs.contains(*path))
		.cloned()
		.chain(local_state.symlinks.iter().cloned())
		.collect::<Vec<_>>();
	file_deletions.sort_unstable();
	file_deletions.dedup();

	let mut directory_creations = desired_dirs
		.iter()
		.filter(|path| !local_state.directories.contains(*path))
		.cloned()
		.collect::<Vec<_>>();
	directory_creations.sort_unstable();

	let mut directory_deletions = local_state
		.directories
		.iter()
		.filter(|path| !desired_dirs.contains(*path))
		.cloned()
		.collect::<Vec<_>>();
	sort_paths_deepest_first(&mut directory_deletions);

	PullActions {
		downloads,
		unchanged,
		file_deletions,
		directory_creations,
		directory_deletions,
	}
}

/// Returns whether a selected remote file should be downloaded.
fn should_download_remote_file(local_hash: Option<&str>, remote_hash: &str, force: bool) -> bool {
	force || local_hash != Some(remote_hash)
}

/// Sorts relative paths deepest-first with lexical tie-breaking.
fn sort_paths_deepest_first(paths: &mut [String]) {
	paths.sort_unstable_by(|left, right| {
		let left_depth = left.bytes().filter(|byte| *byte == b'/').count();
		let right_depth = right.bytes().filter(|byte| *byte == b'/').count();
		right_depth.cmp(&left_depth).then_with(|| left.cmp(right))
	});
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

/// Aborts a pull when its complete mutation plan exceeds the configured safety limit.
fn ensure_pull_action_limit(actions: &PullActions, max_actions: usize) -> Result<()> {
	let action_count = actions
		.downloads
		.len()
		.saturating_add(actions.file_deletions.len())
		.saturating_add(actions.directory_creations.len())
		.saturating_add(actions.directory_deletions.len());
	if action_count > max_actions {
		bail!(
			"Aborting synchronization: {action_count} planned local changes exceeds the limit of {max_actions}.\nIf this is expected, increase `sync.sftp.max_files_to_sync` in your configuration."
		);
	}
	Ok(())
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
fn collect_remote_directories_to_create(actions: &PushActions) -> Vec<String> {
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
		let should_remove = should_remove_for_recreate(sftp.metadata(sftp_path).await, secure_mode);
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

/// Returns true when an existing remote file must be removed before upload.
fn should_remove_for_recreate<E>(
	metadata: CoreResult<FileAttributes, E>,
	secure_mode: u32,
) -> bool {
	metadata.map_or(true, |attrs| {
		attrs
			.permissions
			.map_or_else(|| true, |p| (p & 0o777) != secure_mode)
	})
}

/// Restricts a newly created local pull directory to user-only access.
#[cfg(unix)]
async fn restrict_local_directory_permissions(path: &Path) -> Result<()> {
	async_fs::set_permissions(path, fs::Permissions::from_mode(0o700))
		.await
		.wrap_err_with(|| {
			format!(
				"Failed to set local directory permissions: {}",
				path.display()
			)
		})
}

/// Restricts a newly created local pull directory to user-only access.
#[cfg(not(unix))]
async fn restrict_local_directory_permissions(_path: &Path) -> Result<()> {
	Ok(())
}

/// Ensures the local pull root exists and is not a symlink.
async fn ensure_local_pull_root(path: &Path) -> Result<()> {
	match async_fs::symlink_metadata(path).await {
		Ok(metadata) if metadata.file_type().is_symlink() => {
			bail!(
				"Refusing to use a symlink as the local sync root: {}",
				path.display()
			);
		}
		Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
		Ok(_) => bail!("Local sync root is not a directory: {}", path.display()),
		Err(error) if error.kind() == io::ErrorKind::NotFound => {}
		Err(error) => {
			return Err(error).wrap_err_with(|| {
				format!("Failed to inspect local sync root: {}", path.display())
			});
		}
	}

	async_fs::create_dir_all(path)
		.await
		.wrap_err_with(|| format!("Failed to create local sync root: {}", path.display()))?;
	restrict_local_directory_permissions(path).await
}

/// Ensures a local directory exists under the sync root without traversing symlink components.
async fn ensure_local_directory(root: &Path, relative_path: &str) -> Result<()> {
	let relative_path = checked_relative_path(relative_path)?;
	let mut current = root.to_path_buf();

	for component in relative_path.components() {
		current.push(component.as_os_str());
		match async_fs::symlink_metadata(&current).await {
			Ok(metadata) => {
				let file_type = metadata.file_type();
				if file_type.is_symlink() {
					bail!(
						"Refusing to create local directory through symlink: {}",
						current.display()
					);
				}
				if !file_type.is_dir() {
					bail!(
						"Refusing to create local directory over non-directory: {}",
						current.display()
					);
				}
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {
				async_fs::create_dir(&current).await.wrap_err_with(|| {
					format!("Failed to create local directory: {}", current.display())
				})?;
				restrict_local_directory_permissions(&current).await?;
			}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!("Failed to inspect local directory: {}", current.display())
				});
			}
		}
	}

	Ok(())
}

/// Ensures the parent directory for a local file exists safely under the sync root.
async fn ensure_local_file_parent(root: &Path, relative_path: &str) -> Result<PathBuf> {
	let relative_path = checked_relative_path(relative_path)?;
	if let Some(parent) = relative_path.parent()
		&& !parent.as_os_str().is_empty()
	{
		ensure_local_directory(root, &parent.to_string_lossy()).await?;
	}

	Ok(root.join(relative_path))
}

/// A verified file staged on the destination filesystem.
struct StagedDownload {
	/// Final path relative to the local synchronization root.
	relative_path: String,
	/// Temporary path containing the verified bytes.
	staged_path: PathBuf,
}

/// Creates a private staging directory that does not collide with planned remote paths.
async fn create_pull_staging_directory(
	project_root: &Path,
	actions: &PullActions,
) -> Result<PathBuf> {
	for attempt in 0_u8..100 {
		let name = format!(".biwa-pull-stage-{}-{attempt}", process::id());
		let prefix = format!("{name}/");
		let collides = actions
			.downloads
			.iter()
			.any(|download| download.path == name || download.path.starts_with(&prefix))
			|| actions
				.directory_creations
				.iter()
				.any(|path| path == &name || path.starts_with(&prefix));
		if collides {
			continue;
		}

		let staging_root = project_root.join(name);
		match async_fs::create_dir(&staging_root).await {
			Ok(()) => {
				restrict_local_directory_permissions(&staging_root).await?;
				return Ok(staging_root);
			}
			Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
			Err(error) => {
				return Err(error).wrap_err("Failed to create pull staging directory");
			}
		}
	}

	bail!("Failed to create a unique pull staging directory");
}

/// Applies remote permission bits to a staged local file.
#[cfg(unix)]
async fn apply_download_permissions(
	path: &Path,
	remote_permissions: Option<u32>,
	umask: u32,
) -> Result<()> {
	let mode = remote_permissions.unwrap_or(0o600) & 0o777 & !umask;
	async_fs::set_permissions(path, fs::Permissions::from_mode(mode))
		.await
		.wrap_err_with(|| {
			format!(
				"Failed to set downloaded file permissions: {}",
				path.display()
			)
		})
}

/// Applies remote permission bits to a staged local file.
#[cfg(not(unix))]
async fn apply_download_permissions(
	_path: &Path,
	_remote_permissions: Option<u32>,
	_umask: u32,
) -> Result<()> {
	Ok(())
}

/// Downloads and verifies one remote file inside the private staging tree.
async fn stage_download(
	sftp: &SftpSession,
	remote_path: &str,
	staging_root: &Path,
	download: &Download,
	umask: u32,
) -> Result<StagedDownload> {
	let sftp_path = resolve_sftp_path(remote_path);
	let initial_attributes = sftp
		.symlink_metadata(sftp_path)
		.await
		.wrap_err_with(|| format!("Failed to inspect remote file: {remote_path}"))?;
	if !initial_attributes.is_regular() {
		bail!("Refusing to download non-regular remote file: {remote_path}");
	}

	let mut remote_file = sftp
		.open(sftp_path)
		.await
		.wrap_err_with(|| format!("Failed to open remote file: {remote_path}"))?;
	let staged_path = ensure_local_file_parent(staging_root, &download.path).await?;
	let mut open_options = AsyncOpenOptions::new();
	open_options.write(true).create_new(true);
	#[cfg(unix)]
	open_options.mode(0o600);
	let mut staged_file = open_options
		.open(&staged_path)
		.await
		.wrap_err_with(|| format!("Failed to create staged file: {}", staged_path.display()))?;

	let mut hasher = Sha256::new();
	let mut buffer = vec![0_u8; 64 * 1024];
	loop {
		let read = remote_file
			.read(&mut buffer)
			.await
			.wrap_err_with(|| format!("Failed to read remote file: {remote_path}"))?;
		if read == 0 {
			break;
		}
		let chunk = buffer
			.get(..read)
			.wrap_err("Remote read exceeded the download buffer")?;
		hasher.update(chunk);
		staged_file
			.write_all(chunk)
			.await
			.wrap_err_with(|| format!("Failed to stage remote file: {remote_path}"))?;
	}
	staged_file.flush().await?;
	drop(staged_file);

	let final_attributes = sftp
		.symlink_metadata(sftp_path)
		.await
		.wrap_err_with(|| format!("Failed to re-check remote file: {remote_path}"))?;
	if !final_attributes.is_regular() {
		bail!("Remote file type changed while downloading: {remote_path}");
	}

	let actual_hash = hex::encode(hasher.finalize());
	if actual_hash != download.expected_hash {
		bail!(
			"Remote file changed while downloading: {remote_path} (expected {}, got {actual_hash})",
			download.expected_hash
		);
	}
	apply_download_permissions(&staged_path, final_attributes.permissions, umask).await?;

	Ok(StagedDownload {
		relative_path: download.path.clone(),
		staged_path,
	})
}

/// Deletes local files selected by a pull operation.
async fn delete_local_files(project_root: &Path, relative_paths: &[String]) -> Result<usize> {
	let mut deleted = 0_usize;
	for relative_path in relative_paths {
		let local_path = checked_local_path(project_root, relative_path)?;
		match async_fs::symlink_metadata(&local_path).await {
			Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
				bail!(
					"Refusing to delete local directory as a file: {}",
					local_path.display()
				);
			}
			Ok(_) => {
				async_fs::remove_file(&local_path).await.wrap_err_with(|| {
					format!("Failed to delete local file: {}", local_path.display())
				})?;
				deleted = deleted.saturating_add(1);
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!("Failed to inspect local file: {}", local_path.display())
				});
			}
		}
	}

	Ok(deleted)
}

/// Deletes local directories selected by a pull operation.
async fn delete_local_directories(project_root: &Path, relative_paths: &[String]) -> Result<usize> {
	let mut deleted = 0_usize;
	for relative_path in relative_paths {
		let local_path = checked_local_path(project_root, relative_path)?;
		match async_fs::symlink_metadata(&local_path).await {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				bail!(
					"Refusing to delete local symlink as a directory: {}",
					local_path.display()
				);
			}
			Ok(metadata) if metadata.file_type().is_dir() => {
				match async_fs::remove_dir(&local_path).await {
					Ok(()) => {
						deleted = deleted.saturating_add(1);
					}
					Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {
						warn!(
							path = %local_path.display(),
							"Skipping non-empty local directory selected for pull deletion"
						);
					}
					Err(error) => {
						return Err(error).wrap_err_with(|| {
							format!("Failed to delete local directory: {}", local_path.display())
						});
					}
				}
			}
			Ok(_) => {
				bail!(
					"Refusing to delete local non-directory as a directory: {}",
					local_path.display()
				);
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!(
						"Failed to inspect local directory: {}",
						local_path.display()
					)
				});
			}
		}
	}

	Ok(deleted)
}

/// Target and actions for a synchronization operation.
struct SyncTarget<'a> {
	/// The local project root directory.
	project_root: &'a Path,
	/// The remote directory path.
	remote_dir: &'a str,
	/// The synchronization actions to execute.
	actions: PushActions,
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
	let dirs_to_create = collect_remote_directories_to_create(&actions);
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

/// Executes pull actions by deleting local paths, creating local directories, and downloading files.
async fn apply_pull_actions(
	client: &Client,
	config: &Config,
	project_root: &Path,
	remote_dir: &str,
	actions: PullActions,
	stats: &mut Stats,
	spinner: Option<&ProgressBar>,
) -> Result<()> {
	if actions.file_deletions.is_empty()
		&& actions.directory_creations.is_empty()
		&& actions.directory_deletions.is_empty()
		&& actions.downloads.is_empty()
	{
		return Ok(());
	}

	if actions.downloads.is_empty() {
		return apply_local_pull_actions(project_root, &actions, stats).await;
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

	let staging_root = create_pull_staging_directory(project_root, &actions).await?;
	let total_to_download = actions.downloads.len();
	let stage_result = async {
		let mut staged_downloads = Vec::with_capacity(total_to_download);
		for (index, download) in actions.downloads.iter().enumerate() {
			if let Some(spinner) = spinner {
				spinner.set_message(format!(
					"Downloading files... ({}/{total_to_download})",
					index.saturating_add(1)
				));
			}

			let remote_path = format!("{remote_dir}/{}", download.path);
			staged_downloads.push(
				stage_download(
					&sftp,
					&remote_path,
					&staging_root,
					download,
					config.ssh.umask.as_u32(),
				)
				.await?,
			);
		}
		Ok::<_, color_eyre::Report>(staged_downloads)
	}
	.await;
	let staged_downloads = match stage_result {
		Ok(downloads) => downloads,
		Err(error) => {
			#[expect(clippy::unused_result_ok, reason = "Cleanup failure is secondary")]
			async_fs::remove_dir_all(&staging_root).await.ok();
			return Err(error);
		}
	};

	let commit_result = async {
		apply_local_pull_actions(project_root, &actions, stats).await?;
		for staged in staged_downloads {
			let local_path = ensure_local_file_parent(project_root, &staged.relative_path).await?;
			if let Ok(metadata) = async_fs::symlink_metadata(&local_path).await
				&& metadata.file_type().is_dir()
				&& !metadata.file_type().is_symlink()
			{
				bail!(
					"Refusing to overwrite local directory with remote file: {}",
					local_path.display()
				);
			}
			async_fs::rename(&staged.staged_path, &local_path)
				.await
				.wrap_err_with(|| {
					format!("Failed to commit downloaded file: {}", local_path.display())
				})?;
			stats.downloaded = stats.downloaded.saturating_add(1);
		}
		Ok::<_, color_eyre::Report>(())
	}
	.await;
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is secondary")]
	async_fs::remove_dir_all(&staging_root).await.ok();
	commit_result?;

	Ok(())
}

/// Applies the local filesystem side of a pull operation.
async fn apply_local_pull_actions(
	project_root: &Path,
	actions: &PullActions,
	stats: &mut Stats,
) -> Result<()> {
	stats.deleted = stats
		.deleted
		.saturating_add(delete_local_files(project_root, &actions.file_deletions).await?);
	stats.deleted = stats.deleted.saturating_add(
		delete_local_directories(project_root, &actions.directory_deletions).await?,
	);

	for directory in &actions.directory_creations {
		ensure_local_directory(project_root, directory).await?;
	}

	Ok(())
}

/// Filters remote state to the selected local sync scope for pull operations.
fn filter_remote_state_for_pull(
	remote_state: RemoteState,
	project_root: &Path,
	config_exclude: &[String],
	options: &Options,
) -> Result<RemoteState> {
	let (exclude_globs, include_globs) = build_sync_globsets(config_exclude, options)?;
	let ignore_matcher = build_pull_ignore_matcher(project_root)?;
	let mut filtered = RemoteState::default();

	let mut file_hashes = remote_state.file_hashes.into_iter().collect::<Vec<_>>();
	file_hashes.sort_unstable_by_key(|(path, _)| path.clone());
	for (path, hash) in file_hashes {
		if should_sync_remote_path(
			project_root,
			&path,
			false,
			&ignore_matcher,
			exclude_globs.as_ref(),
			include_globs.as_ref(),
		)? {
			filtered.file_hashes.insert(path, hash);
		}
	}

	let mut directories = remote_state.directories.into_iter().collect::<Vec<_>>();
	directories.sort_unstable();
	for path in directories {
		if should_sync_remote_path(
			project_root,
			&path,
			true,
			&ignore_matcher,
			exclude_globs.as_ref(),
			include_globs.as_ref(),
		)? {
			filtered.directories.insert(path);
		}
	}

	let mut symlinks = remote_state.symlinks.into_iter().collect::<Vec<_>>();
	symlinks.sort_unstable();
	for path in symlinks {
		if should_sync_remote_path(
			project_root,
			&path,
			false,
			&ignore_matcher,
			exclude_globs.as_ref(),
			include_globs.as_ref(),
		)? {
			filtered.symlinks.insert(path);
		}
	}

	Ok(filtered)
}

/// Rejects selected remote symlinks before pulling to avoid ambiguous local writes.
fn ensure_no_remote_symlinks(remote_state: &RemoteState) -> Result<()> {
	if remote_state.symlinks.is_empty() {
		return Ok(());
	}

	let mut symlinks = remote_state.symlinks.iter().cloned().collect::<Vec<_>>();
	symlinks.sort_unstable();
	bail!(
		"Refusing to pull remote symlink entries: {}",
		symlinks.join(", ")
	);
}

/// Shared execution inputs for one sync operation.
struct SyncExecution<'a> {
	/// The SSH client.
	client: &'a Client,
	/// The loaded configuration.
	config: &'a Config,
	/// The local sync root.
	project_root: &'a Path,
	/// The resolved remote project directory.
	remote_dir: &'a str,
	/// Sync options.
	options: &'a Options,
	/// Optional progress spinner.
	spinner: Option<&'a ProgressBar>,
}

/// Plans and applies push synchronization.
async fn execute_push_sync(
	context: &SyncExecution<'_>,
	local_state: &LocalState,
	remote_state: &RemoteState,
	stats: &mut Stats,
) -> Result<()> {
	let actions = calculate_push_actions(local_state, remote_state, context.options);
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
		"Calculated push synchronization actions"
	);

	apply_sync_actions(
		context.client,
		context.config,
		SyncTarget {
			project_root: context.project_root,
			remote_dir: context.remote_dir,
			actions,
		},
		stats,
		context.spinner,
	)
	.await
}

/// Plans and applies pull synchronization.
async fn execute_pull_sync(
	context: &SyncExecution<'_>,
	local_state: &LocalState,
	remote_state: RemoteState,
	stats: &mut Stats,
) -> Result<()> {
	let remote_state = filter_remote_state_for_pull(
		remote_state,
		context.project_root,
		&context.config.sync.exclude,
		context.options,
	)?;
	ensure_no_remote_symlinks(&remote_state)?;
	ensure_sync_file_limit(
		remote_state.file_hashes.len(),
		context.config.sync.sftp.max_files_to_sync,
	)?;
	let actions = calculate_pull_actions(local_state, &remote_state, context.options);
	ensure_pull_action_limit(&actions, context.config.sync.sftp.max_files_to_sync)?;
	stats.unchanged = actions.unchanged;
	info!(
		to_create_dirs = actions.directory_creations.len(),
		to_delete_dirs = actions.directory_deletions.len(),
		to_download = actions.downloads.len(),
		to_delete = actions.file_deletions.len(),
		unchanged = stats.unchanged,
		"Calculated pull synchronization actions"
	);

	apply_pull_actions(
		context.client,
		context.config,
		context.project_root,
		context.remote_dir,
		actions,
		stats,
		context.spinner,
	)
	.await
}

/// Collects local synchronization state without blocking the async runtime.
async fn collect_local_state_async(
	project_root: &Path,
	config: &Config,
	options: &Options,
) -> Result<LocalState> {
	let project_root = project_root.to_path_buf();
	let exclude = config.sync.exclude.clone();
	let options = options.clone();
	spawn_blocking(move || collect_local_state(&project_root, &exclude, &options))
		.await
		.wrap_err("Failed to join blocking task")?
}

/// Synchronizes a project to a remote server.
#[expect(clippy::module_name_repetitions, reason = "No better name exists")]
pub async fn sync_project(
	client: &Client,
	config: &Config,
	project_root: &Path,
	options: &Options,
	direction: Direction,
	remote_dir_override: Option<&str>,
	quiet: bool,
) -> Result<Stats> {
	if config.sync.engine != SyncEngine::Sftp {
		bail!("Only SFTP sync engine is currently supported");
	}
	info!(
			project_root = %project_root.display(),
			direction = ?direction,
			force = options.force,
			include_patterns = options.include.len(),
			exclude_patterns = options.exclude.len(),
			has_remote_override = remote_dir_override.is_some(),
			"Starting project synchronization"
	);

	let spinner = if quiet {
		None
	} else {
		let message = match direction {
			Direction::Push => "Synchronizing files...",
			Direction::Pull => "Downloading files...",
		};
		Some(create_spinner(message.to_owned()))
	};

	// The default remote path hashes the canonical local root, so it must exist first.
	// An explicit pull source can be validated before creating a missing destination.
	if direction == Direction::Pull && remote_dir_override.is_none() {
		ensure_local_pull_root(project_root).await?;
	}
	let remote_dir = match remote_dir_override {
		Some(remote_dir) => remote_dir.to_owned(),
		None => compute_project_remote_dir(config, project_root)?,
	};

	let remote_state =
		fetch_remote_state(client, config, &remote_dir, direction == Direction::Push).await?;
	debug!(
		remote_dir = %remote_dir,
		remote_directories = remote_state.directories.len(),
		remote_files = remote_state.file_hashes.len(),
		remote_symlinks = remote_state.symlinks.len(),
		"Fetched remote sync state"
	);
	if direction == Direction::Pull {
		ensure_local_pull_root(project_root).await?;
	}

	let local_state = collect_local_state_async(project_root, config, options).await?;
	info!(
		local_directories = local_state.directories.len(),
		local_files = local_state.files.len(),
		local_symlinks = local_state.symlinks.len(),
		"Collected local sync state"
	);
	if direction == Direction::Push {
		ensure_sync_file_limit(local_state.files.len(), config.sync.sftp.max_files_to_sync)?;
	}

	let mut stats = Stats::default();
	let execution = SyncExecution {
		client,
		config,
		project_root,
		remote_dir: &remote_dir,
		options,
		spinner: spinner.as_ref(),
	};
	match direction {
		Direction::Push => {
			execute_push_sync(&execution, &local_state, &remote_state, &mut stats).await?;
		}
		Direction::Pull => {
			execute_pull_sync(&execution, &local_state, remote_state, &mut stats).await?;
		}
	}

	if let Some(s) = spinner {
		s.finish_and_clear();
	}
	info!("Sync completed: {:?}", stats);
	if !quiet {
		match direction {
			Direction::Push => {
				eprintln!(
					"{} Sync completed: {} uploaded, {} deleted, {} unchanged",
					style("✓").green().bold(),
					stats.uploaded,
					stats.deleted,
					stats.unchanged
				);
			}
			Direction::Pull => {
				eprintln!(
					"{} Sync completed: {} downloaded, {} deleted, {} unchanged",
					style("✓").green().bold(),
					stats.downloaded,
					stats.deleted,
					stats.unchanged
				);
			}
		}
	}

	Ok(stats)
}

/// Section of the NUL-framed remote inventory protocol.
#[derive(Clone, Copy)]
enum RemoteInventorySection {
	/// Directory paths.
	Directories,
	/// Symlink paths.
	Symlinks,
	/// File hashes and paths.
	Files,
}

/// Parses and validates one remote inventory path.
fn parse_inventory_path(raw_path: &str) -> Result<String> {
	if raw_path.contains('\u{fffd}') {
		bail!("Remote inventory contains a path that is not valid UTF-8");
	}
	let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
	let checked = checked_relative_path(path)?;
	Ok(checked.to_string_lossy().into_owned())
}

/// Returns whether a string is a complete SHA-256 digest.
fn is_sha256_hash(hash: &str) -> bool {
	hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Parses the strict NUL-framed remote inventory.
fn parse_remote_state(output: &str) -> Result<RemoteState> {
	let mut remote_state = RemoteState::default();
	let mut section = None;
	let mut found_sections = HashSet::new();

	for record in output.split_terminator('\0') {
		let marker = match record {
			REMOTE_DIRECTORY_MARKER => Some(RemoteInventorySection::Directories),
			REMOTE_SYMLINK_MARKER => Some(RemoteInventorySection::Symlinks),
			REMOTE_FILE_MARKER => Some(RemoteInventorySection::Files),
			_ => None,
		};
		if let Some(marker) = marker {
			section = Some(marker);
			found_sections.insert(record);
			continue;
		}

		match section.wrap_err("Remote inventory entry appeared before its section marker")? {
			RemoteInventorySection::Directories => {
				remote_state
					.directories
					.insert(parse_inventory_path(record)?);
			}
			RemoteInventorySection::Symlinks => {
				remote_state.symlinks.insert(parse_inventory_path(record)?);
			}
			RemoteInventorySection::Files => {
				let (hash, raw_path) = record
					.split_once("  ")
					.wrap_err("Malformed remote file hash record")?;
				if !is_sha256_hash(hash) {
					bail!("Malformed SHA-256 digest in remote inventory");
				}
				remote_state
					.file_hashes
					.insert(parse_inventory_path(raw_path)?, hash.to_owned());
			}
		}
	}

	for marker in [
		REMOTE_DIRECTORY_MARKER,
		REMOTE_SYMLINK_MARKER,
		REMOTE_FILE_MARKER,
	] {
		if !found_sections.contains(marker) {
			bail!("Remote inventory is missing section marker: {marker}");
		}
	}

	Ok(remote_state)
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use std::fs;
	use tempfile::tempdir;

	/// Builds a download plan entry for planner assertions.
	fn download(path: &str, expected_hash: &str) -> Download {
		Download {
			path: path.to_owned(),
			expected_hash: expected_hash.to_owned(),
		}
	}

	#[test]
	fn should_recreate_file_when_permissions_are_missing_or_different() {
		assert!(should_remove_for_recreate(Err(()), 0o600));
		assert!(should_remove_for_recreate(
			Ok::<FileAttributes, ()>(FileAttributes {
				permissions: None,
				..Default::default()
			}),
			0o600
		));
		assert!(should_remove_for_recreate(
			Ok::<FileAttributes, ()>(FileAttributes {
				permissions: Some(0o100_644),
				..Default::default()
			}),
			0o600
		));
		assert!(!should_remove_for_recreate(
			Ok::<FileAttributes, ()>(FileAttributes {
				permissions: Some(0o100_600),
				..Default::default()
			}),
			0o600
		));
	}

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
	fn collect_local_state_tracks_symlink_entries_without_following_them() {
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
		assert!(state.symlinks.contains("dir-link"));
		assert!(state.symlinks.contains("file-link.txt"));
		assert!(file_paths.contains(&"real-file.txt".to_owned()));
		assert!(!file_paths.contains(&"file-link.txt".to_owned()));
	}

	#[test]
	fn parse_remote_state_rejects_traversal() {
		let hash = "a".repeat(64);
		let output = format!(
			"{REMOTE_DIRECTORY_MARKER}\0{REMOTE_SYMLINK_MARKER}\0{REMOTE_FILE_MARKER}\0{hash}  ./../invalid/path.txt\0"
		);
		let error = parse_remote_state(&output).unwrap_err();
		assert!(error.to_string().contains("unsafe path"), "error: {error}");
	}

	#[test]
	fn parse_remote_state_rejects_absolute_paths() {
		let output = format!(
			"{REMOTE_DIRECTORY_MARKER}\0/etc\0{REMOTE_SYMLINK_MARKER}\0{REMOTE_FILE_MARKER}\0"
		);
		let error = parse_remote_state(&output).unwrap_err();
		assert!(error.to_string().contains("unsafe path"), "error: {error}");
	}

	#[test]
	fn parse_remote_state_collects_nul_framed_entries() {
		let hash = "a".repeat(64);
		let output = format!(
			"{REMOTE_DIRECTORY_MARKER}\0./empty\0./nested/child\0{REMOTE_SYMLINK_MARKER}\0./link\0{REMOTE_FILE_MARKER}\0{hash}  ./nested/file\nname.txt\0"
		);
		let state = parse_remote_state(&output).unwrap();
		assert!(state.directories.contains("empty"));
		assert!(state.directories.contains("nested/child"));
		assert!(state.symlinks.contains("link"));
		assert_eq!(
			state.file_hashes.get("nested/file\nname.txt").unwrap(),
			&hash
		);
	}

	#[test]
	fn parse_remote_state_rejects_malformed_or_incomplete_inventory() {
		let malformed = format!(
			"{REMOTE_DIRECTORY_MARKER}\0{REMOTE_SYMLINK_MARKER}\0{REMOTE_FILE_MARKER}\0not-a-hash  ./file.txt\0"
		);
		let malformed_error = parse_remote_state(&malformed).unwrap_err();
		assert!(
			malformed_error.to_string().contains("Malformed SHA-256"),
			"error: {malformed_error}"
		);
		let incomplete_error = parse_remote_state(REMOTE_DIRECTORY_MARKER).unwrap_err();
		assert!(
			incomplete_error
				.to_string()
				.contains("missing section marker"),
			"error: {incomplete_error}"
		);
	}

	#[test]
	fn build_remote_state_script_creates_only_for_push() {
		let config = Config::default();

		let push_script = build_remote_state_script(&config, "~/project", true);
		assert!(push_script.contains("mkdir -p -- \"$HOME\"/project"));
		assert!(push_script.contains("find . -type d -exec chmod"));
		assert!(push_script.contains("find . -type l -print"));
		assert!(push_script.contains("-print0"));
		assert!(push_script.contains("sha256sum -z"));
		assert!(push_script.contains(REMOTE_DIRECTORY_MARKER));
		assert!(push_script.contains(REMOTE_SYMLINK_MARKER));
		assert!(push_script.contains(REMOTE_FILE_MARKER));

		let pull_script = build_remote_state_script(&config, "~/project", false);
		assert!(!pull_script.contains("mkdir -p --"));
		assert!(!pull_script.contains("find . -type d -exec chmod"));
		assert!(!pull_script.contains("|| true"));
		assert!(pull_script.contains("remote directory does not exist"));
		assert!(pull_script.contains("remote directory is not a directory"));
	}

	#[test]
	fn calculate_push_actions_creates_and_deletes_empty_directories() {
		let actions = calculate_push_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::from(["empty".to_owned()]),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::new(),
				directories: HashSet::from(["stale".to_owned()]),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(actions.directory_creations, vec!["empty".to_owned()]);
		assert_eq!(actions.directory_deletions, vec!["stale".to_owned()]);
		assert!(actions.file_deletions.is_empty());
		assert!(actions.uploads.is_empty());
	}

	#[test]
	fn calculate_push_actions_preserves_directory_when_last_file_removed() {
		let actions = calculate_push_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::from(["dir".to_owned()]),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::from([("dir/file.txt".to_owned(), "hash".to_owned())]),
				directories: HashSet::from(["dir".to_owned()]),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(actions.file_deletions, vec!["dir/file.txt".to_owned()]);
		assert!(actions.directory_deletions.is_empty());
		assert!(actions.directory_creations.is_empty());
	}

	#[test]
	fn calculate_push_actions_deletes_directories_deepest_first() {
		let actions = calculate_push_actions(
			&LocalState::default(),
			&RemoteState {
				file_hashes: HashMap::new(),
				directories: HashSet::from(["a".to_owned(), "a/b".to_owned(), "a/b/c".to_owned()]),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(
			actions.directory_deletions,
			vec!["a/b/c".to_owned(), "a/b".to_owned(), "a".to_owned()]
		);
	}

	#[test]
	fn calculate_pull_actions_downloads_changed_and_missing_files() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: vec![LocalFile {
					path: PathBuf::from("changed.txt"),
					hash: "old".to_owned(),
				}],
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::from([
					("changed.txt".to_owned(), "new".to_owned()),
					("missing.txt".to_owned(), "hash".to_owned()),
				]),
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(
			actions.downloads,
			vec![
				download("changed.txt", "new"),
				download("missing.txt", "hash")
			]
		);
		assert_eq!(actions.unchanged, 0);
		assert!(actions.file_deletions.is_empty());
	}

	#[test]
	fn calculate_push_actions_deletes_remote_files_missing_locally() {
		let actions = calculate_push_actions(
			&LocalState::default(),
			&RemoteState {
				file_hashes: HashMap::from([("stale.txt".to_owned(), "hash".to_owned())]),
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(actions.file_deletions, vec!["stale.txt".to_owned()]);
		assert!(actions.uploads.is_empty());
	}

	#[test]
	fn collect_remote_directories_to_create_includes_upload_parents() {
		let actions = PushActions {
			uploads: vec![PathBuf::from("nested/file.txt")],
			file_deletions: Vec::new(),
			directory_creations: vec!["empty".to_owned()],
			directory_deletions: Vec::new(),
		};

		assert_eq!(
			collect_remote_directories_to_create(&actions),
			vec!["empty".to_owned(), "nested".to_owned()]
		);
	}

	#[test]
	fn calculate_pull_actions_preserves_unchanged_files_unless_forced() {
		let local_state = LocalState {
			files: vec![LocalFile {
				path: PathBuf::from("same.txt"),
				hash: "same".to_owned(),
			}],
			directories: HashSet::new(),
			symlinks: HashSet::new(),
		};
		let remote_state = RemoteState {
			file_hashes: HashMap::from([("same.txt".to_owned(), "same".to_owned())]),
			directories: HashSet::new(),
			symlinks: HashSet::new(),
		};

		let actions = calculate_pull_actions(&local_state, &remote_state, &Options::default());
		assert!(actions.downloads.is_empty());
		assert_eq!(actions.unchanged, 1);

		let actions = calculate_pull_actions(
			&local_state,
			&remote_state,
			&Options {
				force: true,
				..Options::default()
			},
		);
		assert_eq!(actions.downloads, vec![download("same.txt", "same")]);
	}

	#[test]
	fn calculate_pull_actions_mirrors_local_extras_and_empty_dirs() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: vec![LocalFile {
					path: PathBuf::from("stale.txt"),
					hash: "hash".to_owned(),
				}],
				directories: HashSet::from(["old".to_owned()]),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::new(),
				directories: HashSet::from(["empty".to_owned()]),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(actions.file_deletions, vec!["stale.txt".to_owned()]);
		assert_eq!(actions.directory_deletions, vec!["old".to_owned()]);
		assert_eq!(actions.directory_creations, vec!["empty".to_owned()]);
		assert!(actions.downloads.is_empty());
	}

	#[test]
	fn calculate_pull_actions_removes_local_symlinks() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::new(),
				symlinks: HashSet::from(["link".to_owned()]),
			},
			&RemoteState::default(),
			&Options::default(),
		);

		assert_eq!(actions.file_deletions, vec!["link".to_owned()]);
	}

	#[test]
	fn calculate_pull_actions_deletes_directories_deepest_first() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::from(["a".to_owned(), "a/b".to_owned(), "a/b/c".to_owned()]),
				symlinks: HashSet::new(),
			},
			&RemoteState::default(),
			&Options::default(),
		);

		assert_eq!(
			actions.directory_deletions,
			vec!["a/b/c".to_owned(), "a/b".to_owned(), "a".to_owned()]
		);
	}

	#[test]
	fn checked_relative_path_rejects_absolute_and_parent_paths() {
		let parent_error = checked_relative_path("../outside.txt").unwrap_err();
		assert!(
			parent_error
				.to_string()
				.contains("Refusing to synchronize unsafe path"),
			"error: {parent_error}"
		);
		let absolute_error = checked_relative_path("/tmp/outside.txt").unwrap_err();
		assert!(
			absolute_error
				.to_string()
				.contains("Refusing to synchronize unsafe path"),
			"error: {absolute_error}"
		);
		assert_eq!(
			checked_relative_path("./nested/file.txt").unwrap(),
			PathBuf::from("nested/file.txt")
		);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn ensure_local_directory_rejects_symlink_components() {
		use std::os::unix::fs::symlink;

		let dir = tempdir().unwrap();
		let outside = tempdir().unwrap();
		symlink(outside.path(), dir.path().join("link")).unwrap();

		let error = ensure_local_directory(dir.path(), "link/child")
			.await
			.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Refusing to create local directory through symlink"),
			"error: {error}"
		);
		assert!(!outside.path().join("child").exists());
	}

	#[tokio::test]
	async fn ensure_local_file_parent_creates_checked_parent() {
		let dir = tempdir().unwrap();
		let path = ensure_local_file_parent(dir.path(), "nested/file.txt")
			.await
			.unwrap();

		assert_eq!(path, dir.path().join("nested/file.txt"));
		assert!(dir.path().join("nested").is_dir());

		#[cfg(unix)]
		assert_eq!(
			fs::metadata(dir.path().join("nested"))
				.unwrap()
				.permissions()
				.mode() & 0o777,
			0o700
		);
	}

	#[tokio::test]
	async fn create_pull_staging_directory_retries_existing_name() {
		let dir = tempdir().unwrap();
		let attempt_zero = dir
			.path()
			.join(format!(".biwa-pull-stage-{}-0", process::id()));
		fs::create_dir_all(&attempt_zero).unwrap();
		let actions = PullActions {
			downloads: Vec::new(),
			unchanged: 0,
			file_deletions: Vec::new(),
			directory_creations: Vec::new(),
			directory_deletions: Vec::new(),
		};

		let staging_root = create_pull_staging_directory(dir.path(), &actions)
			.await
			.unwrap();
		assert_eq!(
			staging_root,
			dir.path()
				.join(format!(".biwa-pull-stage-{}-1", process::id()))
		);
		assert!(staging_root.is_dir());

		#[cfg(unix)]
		assert_eq!(
			fs::metadata(&staging_root).unwrap().permissions().mode() & 0o777,
			0o700
		);
	}

	#[tokio::test]
	async fn delete_local_files_removes_files_and_rejects_directories() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("stale.txt"), "stale").unwrap();

		let deleted = delete_local_files(
			dir.path(),
			&["stale.txt".to_owned(), "missing.txt".to_owned()],
		)
		.await
		.unwrap();
		assert_eq!(deleted, 1);
		assert!(!dir.path().join("stale.txt").exists());

		fs::create_dir_all(dir.path().join("not-file")).unwrap();
		let error = delete_local_files(dir.path(), &["not-file".to_owned()])
			.await
			.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Refusing to delete local directory as a file"),
			"error: {error}"
		);
	}

	#[tokio::test]
	async fn delete_local_directories_removes_dirs_and_rejects_files() {
		let dir = tempdir().unwrap();
		fs::create_dir_all(dir.path().join("stale")).unwrap();
		fs::create_dir_all(dir.path().join("non-empty")).unwrap();
		fs::write(dir.path().join("non-empty/file.txt"), "kept").unwrap();

		let deleted = delete_local_directories(
			dir.path(),
			&[
				"stale".to_owned(),
				"missing".to_owned(),
				"non-empty".to_owned(),
			],
		)
		.await
		.unwrap();
		assert_eq!(deleted, 1);
		assert!(!dir.path().join("stale").exists());
		assert!(dir.path().join("non-empty/file.txt").exists());

		fs::write(dir.path().join("not-dir"), "file").unwrap();
		let error = delete_local_directories(dir.path(), &["not-dir".to_owned()])
			.await
			.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Refusing to delete local non-directory as a directory"),
			"error: {error}"
		);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn delete_local_directories_rejects_symlinks() {
		use std::os::unix::fs::symlink;

		let dir = tempdir().unwrap();
		let outside = tempdir().unwrap();
		symlink(outside.path(), dir.path().join("link")).unwrap();

		let error = delete_local_directories(dir.path(), &["link".to_owned()])
			.await
			.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Refusing to delete local symlink as a directory"),
			"error: {error}"
		);
		assert!(outside.path().exists());
	}

	#[tokio::test]
	async fn apply_local_pull_actions_updates_local_state_and_stats() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("stale.txt"), "stale").unwrap();
		fs::create_dir_all(dir.path().join("stale-dir")).unwrap();
		let actions = PullActions {
			downloads: vec![download("download.txt", "hash")],
			unchanged: 0,
			file_deletions: vec!["stale.txt".to_owned()],
			directory_creations: vec!["empty/nested".to_owned()],
			directory_deletions: vec!["stale-dir".to_owned()],
		};
		let mut stats = Stats::default();

		apply_local_pull_actions(dir.path(), &actions, &mut stats)
			.await
			.unwrap();

		assert_eq!(stats.deleted, 2);
		assert!(!dir.path().join("stale.txt").exists());
		assert!(!dir.path().join("stale-dir").exists());
		assert!(dir.path().join("empty/nested").is_dir());
	}

	#[test]
	fn ensure_no_remote_symlinks_reports_sorted_entries() {
		ensure_no_remote_symlinks(&RemoteState::default()).unwrap();

		let error = ensure_no_remote_symlinks(&RemoteState {
			file_hashes: HashMap::new(),
			directories: HashSet::new(),
			symlinks: HashSet::from(["z".to_owned(), "a".to_owned()]),
		})
		.unwrap_err();

		assert!(
			error
				.to_string()
				.contains("Refusing to pull remote symlink entries: a, z"),
			"error: {error}"
		);
	}

	#[test]
	fn filter_remote_state_for_pull_respects_include_and_exclude() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join(".gitignore"), "src/ignored.txt\n").unwrap();
		let remote_state = RemoteState {
			file_hashes: HashMap::from([
				("src/main.rs".to_owned(), "hash1".to_owned()),
				("src/ignored.txt".to_owned(), "hash4".to_owned()),
				("target/debug/app".to_owned(), "hash2".to_owned()),
				("README.md".to_owned(), "hash3".to_owned()),
			]),
			directories: HashSet::from(["src".to_owned(), "target".to_owned()]),
			symlinks: HashSet::from(["src/link".to_owned(), "target/link".to_owned()]),
		};

		let filtered = filter_remote_state_for_pull(
			remote_state,
			dir.path(),
			&[dir
				.path()
				.join("target")
				.join("**")
				.to_string_lossy()
				.into_owned()],
			&Options {
				include: vec![
					dir.path()
						.join("src")
						.join("**")
						.to_string_lossy()
						.into_owned(),
				],
				..Options::default()
			},
		)
		.unwrap();

		assert_eq!(filtered.file_hashes.len(), 1);
		assert!(filtered.file_hashes.contains_key("src/main.rs"));
		assert_eq!(filtered.directories, HashSet::from(["src".to_owned()]));
		assert_eq!(filtered.symlinks, HashSet::from(["src/link".to_owned()]));
	}

	#[test]
	fn filter_remote_state_for_pull_respects_nested_ignore_files() {
		let dir = tempdir().unwrap();
		fs::create_dir_all(dir.path().join("src").join("nested")).unwrap();
		fs::write(
			dir.path().join("src").join(".gitignore"),
			"ignored.txt\nignored-dir/\n",
		)
		.unwrap();
		fs::write(
			dir.path().join("src").join(".ignore"),
			"ignored-by-ignore.txt\n",
		)
		.unwrap();
		fs::write(
			dir.path().join("src").join("nested").join(".biwaignore"),
			"!public.txt\nsecret.*\n",
		)
		.unwrap();
		fs::write(
			dir.path().join("src").join("nested").join(".gitignore"),
			"public.txt\n",
		)
		.unwrap();
		let remote_state = RemoteState {
			file_hashes: HashMap::from([
				("src/kept.txt".to_owned(), "hash1".to_owned()),
				("src/ignored.txt".to_owned(), "hash2".to_owned()),
				("src/ignored-dir/file.txt".to_owned(), "hash3".to_owned()),
				("src/ignored-by-ignore.txt".to_owned(), "hash6".to_owned()),
				("src/nested/public.txt".to_owned(), "hash4".to_owned()),
				("src/nested/secret.txt".to_owned(), "hash5".to_owned()),
			]),
			directories: HashSet::from([
				"src".to_owned(),
				"src/ignored-dir".to_owned(),
				"src/nested".to_owned(),
			]),
			symlinks: HashSet::from([
				"src/nested/link".to_owned(),
				"src/nested/secret.link".to_owned(),
			]),
		};

		let filtered =
			filter_remote_state_for_pull(remote_state, dir.path(), &[], &Options::default())
				.unwrap();

		assert_eq!(
			filtered.file_hashes.keys().cloned().collect::<HashSet<_>>(),
			HashSet::from([
				"src/kept.txt".to_owned(),
				"src/nested/public.txt".to_owned()
			])
		);
		assert_eq!(
			filtered.directories,
			HashSet::from(["src".to_owned(), "src/nested".to_owned()])
		);
		assert_eq!(
			filtered.symlinks,
			HashSet::from(["src/nested/link".to_owned()])
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
	fn collect_leaf_directories_handles_string_prefix_siblings() {
		let leaves = collect_leaf_directories(&[
			"a".to_owned(),
			"a-b".to_owned(),
			"a/b".to_owned(),
			"a/b-c".to_owned(),
			"a/b/c".to_owned(),
		]);

		assert_eq!(
			leaves,
			vec!["a/b/c".to_owned(), "a/b-c".to_owned(), "a-b".to_owned()]
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
	fn ensure_pull_action_limit_counts_deletions_and_directories() {
		let actions = PullActions {
			downloads: Vec::new(),
			unchanged: 0,
			file_deletions: vec!["stale.txt".to_owned()],
			directory_creations: vec!["new".to_owned()],
			directory_deletions: Vec::new(),
		};

		ensure_pull_action_limit(&actions, 2).unwrap();
		let error = ensure_pull_action_limit(&actions, 1).unwrap_err();
		assert!(
			error.to_string().contains("2 planned local changes"),
			"error: {error}"
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
	fn is_default_biwa_remote_dir_accepts_expected_layout() {
		let root = Path::new("~/.cache/biwa/projects");
		let hh = "a1b2c3d4";
		let dir = format!(
			"{}/myproj-{hh}-deadbeef",
			root.to_string_lossy().trim_end_matches('/')
		);
		assert!(is_default_biwa_remote_dir(&dir, root, hh));
	}

	#[test]
	fn is_default_biwa_remote_dir_rejects_wrong_root_prefix() {
		let root = Path::new("libs");
		let hh = "a1b2c3d4";
		assert!(!is_default_biwa_remote_dir(
			&format!("libs2/p-{hh}-deadbeef"),
			root,
			hh
		));
	}

	#[test]
	fn is_default_biwa_remote_dir_rejects_nested_path() {
		let root = Path::new("~/r");
		let hh = "a1b2c3d4";
		let r = root.to_string_lossy();
		assert!(!is_default_biwa_remote_dir(
			&format!("{r}/p-{hh}-deadbeef/sub"),
			root,
			hh
		));
	}

	#[test]
	fn is_default_biwa_remote_dir_rejects_path_traversal() {
		let root = Path::new("~/r");
		let hh = "a1b2c3d4";
		let r = root.to_string_lossy();
		assert!(!is_default_biwa_remote_dir(
			&format!("{r}/../other-{hh}-deadbeef"),
			root,
			hh
		));
	}

	#[test]
	fn is_default_biwa_remote_dir_rejects_missing_host_hash() {
		let root = Path::new("~/r");
		let dir = format!(
			"{}/otherproject-nohash-here",
			root.to_string_lossy().trim_end_matches('/')
		);
		assert!(!is_default_biwa_remote_dir(&dir, root, "a1b2c3d4"));
	}

	#[test]
	fn is_default_biwa_remote_dir_rejects_host_hash_inside_project_name_only() {
		let root = Path::new("~/r");
		let hh = "a1b2c3d4";
		let dir = format!("{}/project-{hh}-different-deadbeef", root.to_string_lossy());
		assert!(!is_default_biwa_remote_dir(&dir, root, hh));
	}

	#[test]
	fn is_default_biwa_remote_dir_rejects_invalid_path_hash() {
		let root = Path::new("~/r");
		let hh = "a1b2c3d4";
		let dir = format!("{}/project-{hh}-nothexzz", root.to_string_lossy());
		assert!(!is_default_biwa_remote_dir(&dir, root, hh));
	}

	#[test]
	fn is_biwa_remote_dir_accepts_current_and_legacy_layouts() {
		let root = Path::new("~/r");
		assert!(is_biwa_remote_dir("~/r/project-a1b2c3d4-deadbeef", root));
		assert!(is_biwa_remote_dir("~/r/project-deadbeef", root));
	}

	#[test]
	fn is_biwa_remote_dir_rejects_non_biwa_siblings() {
		let root = Path::new("~/r");
		assert!(!is_biwa_remote_dir("~/r/project", root));
		assert!(!is_biwa_remote_dir("~/r/project-nothexzz", root));
		assert!(!is_biwa_remote_dir("~/r/project-deadbeef/nested", root));
		assert!(!is_biwa_remote_dir("~/r2/project-deadbeef", root));
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
