use super::sync_cache::{
	self, CacheKey, CachedFileState, CachedRemoteState, FileFingerprint, RemoteFingerprint,
	SyncCacheData,
};
#[cfg(test)]
use super::sync_paths::{MAX_REMOTE_MKDIR_COMMAND_LEN, compute_remote_path};
use super::sync_paths::{build_mkdir_commands, collect_leaf_directories, resolve_sftp_path};
use crate::Result;
use crate::config::types::{Config, SftpPermissions, SyncEngine};
use crate::ssh::client::Client;
use crate::ssh::target::ResolvedSshTarget;
use crate::ui::create_spinner;
use alloc::collections::BTreeMap;
use color_eyre::eyre::{Context as _, ContextCompat as _, bail, eyre};
use console::style;
use core::future::{Future, pending};
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
use std::io::Seek as _;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use tokio::fs::{self as async_fs, File as AsyncFile, OpenOptions as AsyncOpenOptions, metadata};
use tokio::io::{
	AsyncReadExt as _, AsyncWriteExt as _, BufReader as AsyncBufReader, copy as async_copy,
};
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tokio::task::{JoinHandle, spawn_blocking, yield_now};
use tracing::{debug, info, warn};

/// Separator emitted by the remote sync-state script before file hash lines.
const REMOTE_FILE_MARKER: &str = "__BIWA_FILE_HASHES__";
/// Separator emitted by the remote sync-state script before file metadata lines.
const REMOTE_FILE_META_MARKER: &str = "__BIWA_FILE_META__";
/// Separator emitted by the remote sync-state script before symlink lines.
const REMOTE_SYMLINK_MARKER: &str = "__BIWA_SYMLINKS__";
/// Separator emitted by the remote sync-state script before directory paths.
const REMOTE_DIRECTORY_MARKER: &str = "__BIWA_DIRECTORIES__";
/// Separator emitted by the remote sync-state script before the remote clock.
const REMOTE_CLOCK_MARKER: &str = "__BIWA_CLOCK__";
/// Reserved top-level prefix for private local pull transactions.
const LOCAL_PULL_STAGE_PREFIX: &str = ".biwa-pull-stage-";
/// Conservative upper bound for a targeted remote hashing command.
///
/// The paths are passed to a shell `printf` rather than to `execve`, but a very
/// long command still risks the remote shell's input limits, and past this size
/// re-hashing the whole remote directory in one pass is cheaper anyway.
const MAX_REMOTE_HASH_COMMAND_LEN: usize = 0x4000;

/// Computes the remote directory path for a given project.
///
/// This is the directory where synced files are stored on the remote server.
pub fn compute_project_remote_dir(config: &Config, project_root: &Path) -> Result<String> {
	super::sync_paths::compute_project_remote_dir(config, project_root)
}

/// Lexically normalizes a remote transfer or cleanup target.
pub fn normalize_remote_dir(remote_dir: &str) -> Result<String> {
	super::sync_paths::normalize_remote_dir(remote_dir)
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

/// Cache-relevant results of one local state scan.
#[derive(Debug, Default)]
struct LocalScanCache {
	/// Cacheable file states keyed by path relative to the sync root.
	entries: BTreeMap<String, CachedFileState>,
	/// Number of files whose hash was reused from the sync cache.
	hits: usize,
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

/// One selected local filesystem entry captured for round-trip conflict checks.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalSnapshotEntry {
	/// A regular file with its content digest and local permission bits.
	File {
		/// SHA-256 content digest.
		hash: String,
		/// Portable Unix permission bits when available.
		mode: Option<u32>,
	},
	/// A directory with its local permission bits.
	Directory {
		/// Portable Unix permission bits when available.
		mode: Option<u32>,
	},
	/// A symbolic link, which round-trip mode preserves but never follows.
	Symlink {
		/// Link target exactly as stored in the directory entry.
		target: PathBuf,
	},
}

/// Selected local state captured immediately after a round-trip push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSnapshot {
	/// Deterministic path-to-entry map relative to the local project root.
	entries: BTreeMap<String, LocalSnapshotEntry>,
}

/// The remote sync state collected from the project directory.
#[derive(Debug, Default, PartialEq, Eq)]
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

/// Returns whether a relative path enters Git's administrative metadata namespace.
fn is_git_metadata_path(path: &Path) -> bool {
	path.components().any(|component| {
		component
			.as_os_str()
			.to_str()
			.is_some_and(|name| name.eq_ignore_ascii_case(".git"))
	})
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
	let relative_path = checked_relative_path(relative_path)?;
	if is_git_metadata_path(&relative_path) {
		return Ok(false);
	}
	let local_equivalent = root.join(relative_path);

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

/// Computes SHA-256 through an already-open readable file handle.
#[cfg(unix)]
fn hash_open_file(file: &File) -> Result<String> {
	let mut file = file
		.try_clone()
		.wrap_err("Failed to clone staged file handle")?;
	file.rewind()
		.wrap_err("Failed to rewind staged file handle")?;
	let mut reader = io::BufReader::new(file);
	let mut hasher = Sha256::new();
	io::copy(
		&mut reader,
		&mut HasherWriter {
			hasher: &mut hasher,
		},
	)
	.wrap_err("Failed to hash installed pull file")?;
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
	if is_git_metadata_path(relative) {
		return Ok(None);
	}
	if relative.components().next().is_some_and(|component| {
		component
			.as_os_str()
			.to_string_lossy()
			.starts_with(LOCAL_PULL_STAGE_PREFIX)
	}) {
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
#[cfg(test)]
fn collect_local_state(
	root: &Path,
	config_exclude: &[String],
	options: &Options,
) -> Result<LocalState> {
	collect_local_state_cached(root, config_exclude, options, None).map(|(state, _)| state)
}

/// Collects local files and directories, reusing cached hashes when possible.
///
/// A cached hash is reused only when the file's metadata fingerprint — size,
/// modification time, and on Unix change time and inode — exactly matches the
/// cached one; every other file is hashed from its content. The returned scan
/// cache holds fingerprinted results eligible for persistence after a
/// successful synchronization.
fn collect_local_state_cached(
	root: &Path,
	config_exclude: &[String],
	options: &Options,
	cached_files: Option<&BTreeMap<String, CachedFileState>>,
) -> Result<(LocalState, LocalScanCache)> {
	let mut builder = WalkBuilder::new(root);
	builder.standard_filters(true); // .gitignore, .ignore, etc.
	builder.add_custom_ignore_filename(".biwaignore");
	builder.hidden(false); // Include hidden files (e.g. .env, .gitignore)
	builder.require_git(false); // Respect .gitignore even outside of git repositories

	let (exclude_globs, include_globs) = build_sync_globsets(config_exclude, options)?;

	let mut state = LocalState::default();
	let mut scan_cache = LocalScanCache::default();
	for entry in builder.build() {
		let entry = entry?;
		let path = entry.path();
		let metadata = fs::symlink_metadata(path)
			.wrap_err_with(|| format!("Failed to read metadata for {}", path.display()))?;
		let file_type = metadata.file_type();
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
			let relative_str = relative.to_string_lossy().into_owned();
			let fingerprint = FileFingerprint::capture(&metadata);
			let cached_hash = fingerprint.and_then(|fingerprint| {
				cached_files?
					.get(&relative_str)
					.filter(|cached| cached.fingerprint == fingerprint)
					.map(|cached| cached.hash.clone())
			});
			let hash = match cached_hash {
				Some(hash) => {
					scan_cache.hits = scan_cache.hits.saturating_add(1);
					hash
				}
				None => hash_file(path)?,
			};
			if let Some(fingerprint) = fingerprint {
				scan_cache.entries.insert(
					relative_str,
					CachedFileState {
						fingerprint,
						hash: hash.clone(),
					},
				);
			}
			state.files.push(LocalFile {
				path: relative,
				hash,
			});
			continue;
		}

		if is_dir {
			state
				.directories
				.insert(relative.to_string_lossy().into_owned());
		}
	}

	Ok((state, scan_cache))
}

/// Returns local permission bits used to detect and preserve round-trip changes.
#[cfg(unix)]
fn local_permission_mode(path: &Path) -> Result<Option<u32>> {
	let metadata = fs::symlink_metadata(path)
		.wrap_err_with(|| format!("Failed to read metadata for {}", path.display()))?;
	Ok(Some(metadata.permissions().mode() & 0o777))
}

/// Returns local permission bits used to detect and preserve round-trip changes.
#[cfg(not(unix))]
fn local_permission_mode(_path: &Path) -> Result<Option<u32>> {
	Ok(None)
}

/// Converts collected local state into a deterministic round-trip snapshot.
fn snapshot_from_local_state(project_root: &Path, state: &LocalState) -> Result<LocalSnapshot> {
	let mut entries = BTreeMap::new();
	for file in &state.files {
		let path = file.path.to_string_lossy().into_owned();
		entries.insert(
			path,
			LocalSnapshotEntry::File {
				hash: file.hash.clone(),
				mode: local_permission_mode(&project_root.join(&file.path))?,
			},
		);
	}
	let mut directories = state.directories.iter().collect::<Vec<_>>();
	directories.sort_unstable();
	for directory in directories {
		entries.insert(
			directory.clone(),
			LocalSnapshotEntry::Directory {
				mode: local_permission_mode(&project_root.join(directory))?,
			},
		);
	}
	let mut symlinks = state.symlinks.iter().collect::<Vec<_>>();
	symlinks.sort_unstable();
	for symlink in symlinks {
		let local_path = project_root.join(symlink);
		entries.insert(
			symlink.clone(),
			LocalSnapshotEntry::Symlink {
				target: fs::read_link(&local_path).wrap_err_with(|| {
					format!("Failed to read local symlink: {}", local_path.display())
				})?,
			},
		);
	}
	Ok(LocalSnapshot { entries })
}

/// Returns paths whose selected local state differs between two snapshots.
fn changed_snapshot_paths(expected: &LocalSnapshot, actual: &LocalSnapshot) -> Vec<String> {
	let mut paths = expected
		.entries
		.keys()
		.chain(actual.entries.keys())
		.filter(|path| expected.entries.get(*path) != actual.entries.get(*path))
		.cloned()
		.collect::<Vec<_>>();
	paths.sort_unstable();
	paths.dedup();
	paths
}

/// Rejects a round-trip pull when selected local state changed after the push.
fn ensure_local_snapshot_matches(
	expected: &LocalSnapshot,
	actual: &LocalSnapshot,
	phase: &str,
) -> Result<()> {
	let changed_paths = changed_snapshot_paths(expected, actual);
	if changed_paths.is_empty() {
		return Ok(());
	}

	let displayed = changed_paths.iter().take(10).cloned().collect::<Vec<_>>();
	let suffix = if changed_paths.len() > displayed.len() {
		format!(
			" (and {} more)",
			changed_paths.len().saturating_sub(displayed.len())
		)
	} else {
		String::new()
	};
	bail!(
		"Local files changed {phase}; refusing to overwrite them during round-trip pull: {}{suffix}",
		displayed.join(", ")
	)
}

/// Rejects a workflow when selected local state changed between two snapshots.
pub fn ensure_local_snapshot_unchanged(
	expected: &LocalSnapshot,
	actual: &LocalSnapshot,
	phase: &str,
) -> Result<()> {
	ensure_local_snapshot_matches(expected, actual, phase)
}

/// Captures selected local state for round-trip synchronization.
pub async fn snapshot_local_project(
	project_root: &Path,
	config: &Config,
	options: &Options,
) -> Result<LocalSnapshot> {
	let (state, _) = collect_local_state_async(project_root, config, options, None).await?;
	snapshot_from_local_state(project_root, &state)
}

/// Verifies that a completed round-trip push exactly represents its local baseline remotely.
pub async fn ensure_remote_matches_local_snapshot(
	client: &Client,
	config: &Config,
	remote_dir: &str,
	baseline: &LocalSnapshot,
) -> Result<()> {
	let mut expected = RemoteState::default();
	for (path, entry) in &baseline.entries {
		match entry {
			LocalSnapshotEntry::File { hash, .. } => {
				expected.file_hashes.insert(path.clone(), hash.clone());
			}
			LocalSnapshotEntry::Directory { .. } => {
				expected.directories.insert(path.clone());
			}
			LocalSnapshotEntry::Symlink { .. } => {}
		}
	}
	collect_parent_directories_into(
		expected.file_hashes.keys().map(Path::new),
		&mut expected.directories,
	);

	// Verification passes never consult the remote cache: they exist to prove
	// the remote content itself, so every file is hashed again.
	let (actual, _) = fetch_remote_state(client, config, remote_dir, false, None).await?;
	if actual != expected {
		bail!(
			"Remote project does not exactly match the completed push; refusing to run a round-trip command"
		);
	}
	Ok(())
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

/// How much remote file state one inventory run collects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InventoryMode {
	/// Hash every remote file, completing a full revalidation pass.
	Full,
	/// Collect metadata only and hash just the files the cache cannot explain.
	Incremental,
}

/// One remote inventory exactly as the remote script reported it.
#[derive(Debug, Default)]
struct RemoteInventory {
	/// Remote clock in Unix seconds captured before the directory was scanned.
	clock: i64,
	/// The remote directories that currently exist.
	directories: HashSet<String>,
	/// The remote symlinks that currently exist.
	symlinks: HashSet<String>,
	/// Remote file metadata keyed by path relative to the remote directory.
	///
	/// A `None` fingerprint marks a file whose metadata cannot serve as a cache
	/// key — a pre-epoch timestamp, for instance. The path stays listed so the
	/// file is still part of the remote state; only its hash is never cached.
	metadata: BTreeMap<String, Option<RemoteFingerprint>>,
	/// Remote file hashes keyed by path relative to the remote directory.
	hashes: HashMap<String, String>,
}

/// Cache-relevant results of one remote inventory run.
#[derive(Debug, Default)]
struct RemoteScanCache {
	/// Cacheable remote file states keyed by path relative to the remote directory.
	entries: BTreeMap<String, CachedRemoteState>,
	/// Number of remote files whose hash was reused from the sync cache.
	hits: usize,
	/// Number of remote files hashed by the remote server during this run.
	hashed: usize,
	/// Whether this run hashed every remote file.
	full_hash: bool,
}

/// Builds the remote shell script that emits directory, symlink, and file state.
///
/// The metadata listing always precedes the hash listing so a file rewritten
/// between the two is recorded with its older fingerprint and its newer hash.
/// The next run then sees a fingerprint mismatch and re-hashes it, which is the
/// safe direction; the reverse order could hide the change behind a fingerprint
/// that matches a stale hash.
fn build_remote_state_script(
	config: &Config,
	remote_dir: &str,
	create_remote_dir: bool,
	mode: InventoryMode,
) -> String {
	let quoted_remote_dir = shell_quote_path(remote_dir);
	let quoted_marker = shell_words::quote(REMOTE_FILE_MARKER).into_owned();
	let quoted_meta_marker = shell_words::quote(REMOTE_FILE_META_MARKER).into_owned();
	let quoted_symlink_marker = shell_words::quote(REMOTE_SYMLINK_MARKER).into_owned();
	let quoted_directory_marker = shell_words::quote(REMOTE_DIRECTORY_MARKER).into_owned();
	let quoted_clock_marker = shell_words::quote(REMOTE_CLOCK_MARKER).into_owned();
	let hash_files = match mode {
		InventoryMode::Full => " && find . -iname .git -prune -o -type f -exec sha256sum -z {} +",
		InventoryMode::Incremental => "",
	};
	let prepare_remote_dir = if create_remote_dir {
		format!("mkdir -p -- {quoted_remote_dir} &&")
	} else {
		format!(
			"if [ ! -e {quoted_remote_dir} ]; then echo 'Error: remote directory does not exist' >&2; exit 1; fi &&"
		)
	};
	let normalize_remote_dirs = if create_remote_dir {
		let dir_mode = format!("{:04o}", 0o777 & !config.ssh.umask.as_u32());
		format!(
			"(chmod {dir_mode} . && find . -mindepth 1 -iname .git -prune -o -type d -exec chmod {dir_mode} {{}} + || true) &&"
		)
	} else {
		String::new()
	};

	// Prepare the remote dir, normalize owned push directories, then print the
	// remote clock, directories, symlinks, file metadata, and — for a full
	// inventory — file hashes, without following symlinks.
	format!(
		"umask {} && {prepare_remote_dir} \
		 if [ -L {quoted_remote_dir} ]; then echo 'Error: remote directory is a symlink' >&2; exit 1; fi && \
		 if [ ! -d {quoted_remote_dir} ]; then echo 'Error: remote directory is not a directory' >&2; exit 1; fi && \
		 cd -- {quoted_remote_dir} && \
		 {normalize_remote_dirs} \
		 printf '%s\\0' {quoted_clock_marker} && \
		 printf '%s\\0' \"$(date +%s)\" && \
		 printf '%s\\0' {quoted_directory_marker} && \
		 find . -mindepth 1 -iname .git -prune -o -type d -print0 && \
		 printf '%s\\0' {quoted_symlink_marker} && \
		 find . -iname .git -prune -o -type l -print0 && \
		 printf '%s\\0' {quoted_meta_marker} && \
		 find . -iname .git -prune -o -type f -printf '%s %T@ %C@ %p\\0' && \
		 printf '%s\\0' {quoted_marker}{hash_files}",
		config.ssh.umask
	)
}

/// Builds a remote command that hashes exactly the given relative paths.
///
/// The paths are streamed into `xargs` through a shell `printf`, which keeps
/// paths containing spaces or newlines intact and lets `xargs` batch the
/// invocations. Returns `None` when the command would grow past
/// [`MAX_REMOTE_HASH_COMMAND_LEN`].
fn build_remote_hash_command(remote_dir: &str, paths: &[String]) -> Option<String> {
	/// Pipeline appended once every path has been added.
	const HASH_COMMAND_SUFFIX: &str = " | xargs -0 sha256sum -z --";

	let mut command = format!("cd -- {} && printf '%s\\0'", shell_quote_path(remote_dir));
	for path in paths {
		// The `./` prefix keeps a path that starts with `-` from looking like an
		// option, and matches the paths the inventory itself reports.
		command.push(' ');
		command.push_str(&shell_words::quote(&format!("./{path}")));
		// The suffix is counted here so the finished command, not the partial
		// one, is what the bound applies to.
		if command.len().saturating_add(HASH_COMMAND_SUFFIX.len()) > MAX_REMOTE_HASH_COMMAND_LEN {
			return None;
		}
	}
	command.push_str(HASH_COMMAND_SUFFIX);
	Some(command)
}

/// Runs one remote inventory script and parses its output.
async fn run_remote_inventory(
	client: &Client,
	config: &Config,
	remote_dir: &str,
	create_remote_dir: bool,
	mode: InventoryMode,
) -> Result<RemoteInventory> {
	let script = build_remote_state_script(config, remote_dir, create_remote_dir, mode);

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

	parse_remote_inventory(&output)
}

/// Returns whether a cached entry matches an inventoried file's metadata.
///
/// A file without a usable fingerprint never matches, so it is always hashed
/// again rather than resolved from the cache.
fn cached_remote_hash<'cache>(
	cached: Option<&'cache BTreeMap<String, CachedRemoteState>>,
	path: &str,
	fingerprint: Option<&RemoteFingerprint>,
) -> Option<&'cache str> {
	let fingerprint = fingerprint?;
	cached?
		.get(path)
		.filter(|entry| &entry.fingerprint == fingerprint)
		.map(|entry| entry.hash.as_str())
}

/// Returns the inventoried paths whose content hash is still unknown.
fn pending_hash_paths(
	inventory: &RemoteInventory,
	cached: Option<&BTreeMap<String, CachedRemoteState>>,
) -> Vec<String> {
	inventory
		.metadata
		.iter()
		.filter(|(path, fingerprint)| {
			!inventory.hashes.contains_key(*path)
				&& cached_remote_hash(cached, path, fingerprint.as_ref()).is_none()
		})
		.map(|(path, _)| path.clone())
		.collect()
}

/// Hashes the remote files an incremental inventory could not resolve from cache.
///
/// Returns `false` when the targeted pass could not be completed, in which case
/// the caller falls back to a full inventory. Every failure is a fallback
/// rather than an error: the full pass produces the same state, only slower.
async fn hash_pending_remote_files(
	client: &Client,
	remote_dir: &str,
	inventory: &mut RemoteInventory,
	pending: &[String],
) -> bool {
	let Some(command) = build_remote_hash_command(remote_dir, pending) else {
		debug!(
			pending = pending.len(),
			"Targeted remote hash command is too large; falling back to a full remote hash pass"
		);
		return false;
	};
	let result = match client.execute(&command).await {
		Ok(result) => result,
		Err(error) => {
			warn!(%error, "Failed to run targeted remote hashing; falling back to a full remote hash pass");
			return false;
		}
	};
	if result.exit_status != 0 {
		warn!(
			exit_status = result.exit_status,
			stderr = result.stderr.trim(),
			"Targeted remote hashing failed; falling back to a full remote hash pass"
		);
		return false;
	}
	match parse_remote_hashes(&result.stdout) {
		Ok(hashes) => {
			inventory.hashes.extend(hashes);
			// Every requested path must now have a hash. `xargs` reports a failed
			// `sha256sum` with a non-zero status, so a short result is not
			// expected, but checking it here keeps a file from being resolved
			// from a hash nothing produced.
			let resolved = pending
				.iter()
				.all(|path| inventory.hashes.contains_key(path));
			if !resolved {
				warn!(
					requested = pending.len(),
					"Targeted remote hashing returned fewer hashes than requested; falling back to a full remote hash pass"
				);
			}
			resolved
		}
		Err(error) => {
			warn!(%error, "Failed to parse targeted remote hashes; falling back to a full remote hash pass");
			false
		}
	}
}

/// Builds the remote sync state and its cache entries from a resolved inventory.
///
/// A file listed with metadata but without a hash disappeared between the two
/// remote passes and is dropped; a file with a hash but no metadata appeared
/// between them and is kept out of the cache instead.
fn build_remote_state(
	inventory: RemoteInventory,
	cached: Option<&BTreeMap<String, CachedRemoteState>>,
	mode: InventoryMode,
) -> (RemoteState, RemoteScanCache) {
	let RemoteInventory {
		clock,
		directories,
		symlinks,
		metadata,
		mut hashes,
	} = inventory;
	let mut scan = RemoteScanCache {
		full_hash: mode == InventoryMode::Full,
		..RemoteScanCache::default()
	};
	let mut file_hashes = HashMap::new();
	for (path, fingerprint) in metadata {
		let hash = if let Some(hash) = hashes.remove(&path) {
			scan.hashed = scan.hashed.saturating_add(1);
			hash
		} else if let Some(hash) = cached_remote_hash(cached, &path, fingerprint.as_ref()) {
			scan.hits = scan.hits.saturating_add(1);
			hash.to_owned()
		} else {
			debug!(
				path,
				"Remote file vanished before it could be hashed; ignoring it"
			);
			continue;
		};
		if let Some(fingerprint) = fingerprint.filter(|fingerprint| fingerprint.is_settled(clock)) {
			scan.entries.insert(
				path.clone(),
				CachedRemoteState {
					fingerprint,
					hash: hash.clone(),
				},
			);
		}
		file_hashes.insert(path, hash);
	}
	file_hashes.extend(hashes);

	(
		RemoteState {
			file_hashes,
			directories,
			symlinks,
		},
		scan,
	)
}

/// Fetches the current remote directory and file state.
///
/// With a usable remote cache, the inventory only lists file metadata and just
/// the files whose fingerprint changed are hashed remotely; otherwise every
/// remote file is hashed in one pass. The returned scan cache holds the entries
/// eligible for persistence after a successful synchronization.
async fn fetch_remote_state(
	client: &Client,
	config: &Config,
	remote_dir: &str,
	create_remote_dir: bool,
	cached_remote: Option<&BTreeMap<String, CachedRemoteState>>,
) -> Result<(RemoteState, RemoteScanCache)> {
	let full_inventory = async || {
		run_remote_inventory(
			client,
			config,
			remote_dir,
			create_remote_dir,
			InventoryMode::Full,
		)
		.await
		.map(|inventory| build_remote_state(inventory, None, InventoryMode::Full))
	};
	let Some(cached) = cached_remote.filter(|entries| !entries.is_empty()) else {
		return full_inventory().await;
	};

	let mut inventory = run_remote_inventory(
		client,
		config,
		remote_dir,
		create_remote_dir,
		InventoryMode::Incremental,
	)
	.await?;
	let pending = pending_hash_paths(&inventory, Some(cached));
	if !pending.is_empty()
		&& !hash_pending_remote_files(client, remote_dir, &mut inventory, &pending).await
	{
		return full_inventory().await;
	}
	Ok(build_remote_state(
		inventory,
		Some(cached),
		InventoryMode::Incremental,
	))
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
	to_delete_files.extend(remote_state.symlinks.iter().cloned());

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
	preserve_local_symlinks: bool,
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
		.chain(
			local_state
				.symlinks
				.iter()
				.filter(|_| !preserve_local_symlinks)
				.cloned(),
		)
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
) -> Result<usize> {
	let mut deleted = 0_usize;
	for path in relative_paths {
		let full_path = format!("{remote_dir}/{path}");
		let sftp_path = resolve_sftp_path(&full_path);
		sftp.remove_dir(sftp_path)
			.await
			.wrap_err_with(|| format!("Failed to delete remote directory: {sftp_path}"))?;
		deleted = deleted.saturating_add(1);
	}

	Ok(deleted)
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

/// Ensures a local directory exists and records each created component immediately.
async fn ensure_local_directory_recording(
	root: &Path,
	relative_path: &str,
	created_directories: &mut Vec<PathBuf>,
) -> Result<()> {
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
				created_directories.push(current.clone());
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

/// Ensures a local directory exists under the sync root without traversing symlink components.
#[cfg(test)]
async fn ensure_local_directory(root: &Path, relative_path: &str) -> Result<()> {
	ensure_local_directory_recording(root, relative_path, &mut Vec::new()).await
}

/// Ensures the parent directory for a local file exists safely under the sync root.
async fn ensure_local_file_parent(root: &Path, relative_path: &str) -> Result<PathBuf> {
	ensure_local_file_parent_recording(root, relative_path, &mut Vec::new()).await
}

/// Ensures a local file parent exists while recording created directories for rollback.
async fn ensure_local_file_parent_recording(
	root: &Path,
	relative_path: &str,
	created_directories: &mut Vec<PathBuf>,
) -> Result<PathBuf> {
	let relative_path = checked_relative_path(relative_path)?;
	if let Some(parent) = relative_path.parent()
		&& !parent.as_os_str().is_empty()
	{
		ensure_local_directory_recording(root, &parent.to_string_lossy(), created_directories)
			.await?;
	}

	Ok(root.join(relative_path))
}

/// A verified file staged on the destination filesystem.
struct StagedDownload {
	/// Final path relative to the local synchronization root.
	relative_path: String,
	/// Temporary path containing the verified bytes.
	staged_path: PathBuf,
	/// Verified content and final permission fingerprint.
	fingerprint: LocalSnapshotEntry,
	/// Readable handle retained across restrictive permission changes for safe rollback.
	#[cfg(unix)]
	rollback_file: Option<File>,
}

/// One local entry moved aside during a transactional pull commit.
struct PullBackup {
	/// Original path relative to the local synchronization root.
	relative_path: String,
	/// Original local path.
	original_path: PathBuf,
	/// Private backup path on the same filesystem.
	backup_path: PathBuf,
	/// Whether the action is counted as a mirror deletion rather than replacement.
	counts_as_deletion: bool,
}

/// One staged file installed during a transactional pull commit.
struct PullInstalledFile {
	/// Final local path.
	path: PathBuf,
	/// Fingerprint captured immediately after installation.
	fingerprint: LocalSnapshotEntry,
	/// Readable handle to the installed inode, retained for rollback verification.
	#[cfg(unix)]
	rollback_file: File,
}

/// Mutable state needed to roll back a pull commit.
#[derive(Default)]
struct PullCommitState {
	/// Existing entries moved into the private backup directory.
	backups: Vec<PullBackup>,
	/// Newly installed downloads.
	installed_files: Vec<PullInstalledFile>,
	/// Newly created directories, ordered from shallowest to deepest.
	created_directories: Vec<PathBuf>,
}

/// Fingerprints one local entry without following symbolic links.
fn fingerprint_local_entry(path: &Path) -> Result<Option<LocalSnapshotEntry>> {
	let metadata = match fs::symlink_metadata(path) {
		Ok(metadata) => metadata,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
		Err(error) => {
			return Err(error)
				.wrap_err_with(|| format!("Failed to inspect local entry: {}", path.display()));
		}
	};
	let file_type = metadata.file_type();
	if file_type.is_symlink() {
		return Ok(Some(LocalSnapshotEntry::Symlink {
			target: fs::read_link(path)
				.wrap_err_with(|| format!("Failed to read local symlink: {}", path.display()))?,
		}));
	}
	#[expect(
		clippy::filetype_is_file,
		reason = "pull fingerprints support only regular files, directories, and symlinks"
	)]
	if file_type.is_file() {
		return Ok(Some(LocalSnapshotEntry::File {
			hash: hash_file(path)?,
			mode: local_permission_mode(path)?,
		}));
	}
	if file_type.is_dir() {
		return Ok(Some(LocalSnapshotEntry::Directory {
			mode: local_permission_mode(path)?,
		}));
	}
	bail!("Unsupported local entry type: {}", path.display())
}

/// Creates a private directory with restrictive permissions in the creation syscall.
fn create_private_local_directory(path: &Path) -> io::Result<()> {
	let mut builder = fs::DirBuilder::new();
	#[cfg(unix)]
	builder.mode(0o700);
	builder.create(path)
}

/// Removes a private pull transaction directory, accepting an already-absent path.
async fn remove_pull_staging_directory(path: &Path) -> Result<()> {
	match async_fs::remove_dir_all(path).await {
		Ok(()) => Ok(()),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(error).wrap_err_with(|| {
			format!("Failed to remove pull transaction data: {}", path.display())
		}),
	}
}

/// Creates a private staging directory that does not collide with planned remote paths.
fn create_pull_staging_directory(project_root: &Path) -> Result<PathBuf> {
	for _ in 0_u8..100 {
		let mut random = [0_u8; 16];
		getrandom::fill(&mut random).wrap_err("Failed to generate a pull transaction name")?;
		let name = format!("{LOCAL_PULL_STAGE_PREFIX}{}", hex::encode(random));
		let staging_root = project_root.join(name);
		match create_private_local_directory(&staging_root) {
			Ok(()) => return Ok(staging_root),
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
) -> Result<Option<u32>> {
	let mode = remote_permissions.unwrap_or(0o600) & 0o777 & !umask;
	async_fs::set_permissions(path, fs::Permissions::from_mode(mode))
		.await
		.wrap_err_with(|| {
			format!(
				"Failed to set downloaded file permissions: {}",
				path.display()
			)
		})?;
	Ok(Some(mode))
}

/// Applies remote permission bits to a staged local file.
#[cfg(not(unix))]
async fn apply_download_permissions(
	_path: &Path,
	_remote_permissions: Option<u32>,
	_umask: u32,
) -> Result<Option<u32>> {
	Ok(None)
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
	open_options.read(true).write(true).create_new(true);
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
	#[cfg(unix)]
	let rollback_file = staged_file.into_std().await;
	#[cfg(not(unix))]
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
	let mode =
		apply_download_permissions(&staged_path, final_attributes.permissions, umask).await?;

	Ok(StagedDownload {
		relative_path: download.path.clone(),
		staged_path,
		fingerprint: LocalSnapshotEntry::File {
			hash: actual_hash,
			mode,
		},
		#[cfg(unix)]
		rollback_file: Some(rollback_file),
	})
}

/// Restores the pre-command local mode for an existing round-trip file.
#[cfg(unix)]
async fn preserve_round_trip_file_mode(
	staged: &mut StagedDownload,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	let Some(LocalSnapshotEntry::File {
		mode: Some(mode), ..
	}) = baseline.and_then(|snapshot| snapshot.entries.get(&staged.relative_path))
	else {
		return Ok(());
	};
	async_fs::set_permissions(&staged.staged_path, fs::Permissions::from_mode(*mode))
		.await
		.wrap_err_with(|| {
			format!(
				"Failed to preserve local file permissions: {}",
				staged.relative_path
			)
		})?;
	let LocalSnapshotEntry::File {
		mode: staged_mode, ..
	} = &mut staged.fingerprint
	else {
		bail!("Staged pull entry is not a regular file")
	};
	*staged_mode = Some(*mode);
	Ok(())
}

/// Restores the pre-command local mode for an existing round-trip file.
#[cfg(not(unix))]
async fn preserve_round_trip_file_mode(
	_staged: &mut StagedDownload,
	_baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	Ok(())
}

/// Deletes local files selected by a pull operation.
#[cfg(test)]
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
#[cfg(test)]
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
		sftp.remove_file(sftp_path)
			.await
			.wrap_err_with(|| format!("Failed to delete remote file: {sftp_path}"))?;
		stats.deleted = stats.deleted.saturating_add(1);
	}

	// Remove deleted directories deepest-first so parents become empty first.
	stats.deleted = stats.deleted.saturating_add(
		delete_remote_directories(&sftp, remote_dir, &actions.directory_deletions).await?,
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

/// Moves one existing local entry into the private transaction backup.
async fn backup_local_entry(
	state: &mut PullCommitState,
	relative_path: &str,
	original_path: PathBuf,
	backup_root: &Path,
	counts_as_deletion: bool,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	let backup_path = backup_root.join(state.backups.len().to_string());
	async_fs::rename(&original_path, &backup_path)
		.await
		.wrap_err_with(|| format!("Failed to back up local entry: {}", original_path.display()))?;
	state.backups.push(PullBackup {
		relative_path: relative_path.to_owned(),
		original_path,
		backup_path: backup_path.clone(),
		counts_as_deletion,
	});
	if let Some(baseline) = baseline {
		let expected = baseline.entries.get(relative_path);
		let actual = fingerprint_local_entry(&backup_path)?;
		if actual.as_ref() != expected {
			bail!(
				"Local entry changed immediately before pull commit; refusing to overwrite it: {relative_path}"
			);
		}
	}
	Ok(())
}

/// Revalidates backed-up entries immediately before their recovery copies may be destroyed.
async fn ensure_pull_backups_unchanged(
	state: &PullCommitState,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	let Some(baseline) = baseline else {
		return Ok(());
	};

	for backup in &state.backups {
		let expected = baseline.entries.get(&backup.relative_path);
		let actual = fingerprint_local_entry(&backup.backup_path)?;
		if actual.as_ref() != expected {
			bail!(
				"Local entry changed during pull commit; refusing to discard its recovery copy: {}",
				backup.relative_path
			);
		}
		if matches!(expected, Some(LocalSnapshotEntry::Directory { .. }))
			&& !local_directory_is_empty(&backup.backup_path).await?
		{
			bail!(
				"Local directory changed during pull commit; refusing to discard its recovery copy: {}",
				backup.relative_path
			);
		}
	}

	Ok(())
}

/// Backs up files, symlinks, and file replacements before a pull commit.
async fn backup_pull_file_targets(
	project_root: &Path,
	backup_root: &Path,
	actions: &PullActions,
	state: &mut PullCommitState,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	let deletion_paths = actions
		.file_deletions
		.iter()
		.cloned()
		.collect::<HashSet<_>>();
	let mut paths = actions
		.file_deletions
		.iter()
		.cloned()
		.chain(
			actions
				.downloads
				.iter()
				.map(|download| download.path.clone()),
		)
		.collect::<Vec<_>>();
	paths.sort_unstable();
	paths.dedup();

	for relative_path in paths {
		let local_path = checked_local_path(project_root, &relative_path)?;
		match async_fs::symlink_metadata(&local_path).await {
			Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
				// File-vs-directory replacements are handled with directory targets below.
			}
			Ok(_) => {
				backup_local_entry(
					state,
					&relative_path,
					local_path,
					backup_root,
					deletion_paths.contains(&relative_path),
					baseline,
				)
				.await?;
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!("Failed to inspect local entry: {}", local_path.display())
				});
			}
		}
	}
	Ok(())
}

/// Returns whether a local directory currently has no entries.
async fn local_directory_is_empty(path: &Path) -> Result<bool> {
	let mut entries = async_fs::read_dir(path)
		.await
		.wrap_err_with(|| format!("Failed to read local directory: {}", path.display()))?;
	Ok(entries.next_entry().await?.is_none())
}

/// Backs up empty directories selected for deletion or file replacement.
async fn backup_pull_directory_targets(
	project_root: &Path,
	backup_root: &Path,
	actions: &PullActions,
	state: &mut PullCommitState,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	let replacement_paths = actions
		.downloads
		.iter()
		.map(|download| download.path.as_str())
		.collect::<HashSet<_>>();
	for relative_path in &actions.directory_deletions {
		let local_path = checked_local_path(project_root, relative_path)?;
		match async_fs::symlink_metadata(&local_path).await {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				bail!(
					"Refusing to replace local symlink as a directory: {}",
					local_path.display()
				);
			}
			Ok(metadata) if metadata.file_type().is_dir() => {
				if !local_directory_is_empty(&local_path).await? {
					if baseline.is_some() {
						bail!(
							"Local directory changed immediately before pull commit; refusing to overwrite it: {}",
							local_path.display()
						);
					}
					if replacement_paths.contains(relative_path.as_str()) {
						bail!(
							"Refusing to overwrite non-empty local directory with remote file: {}",
							local_path.display()
						);
					}
					warn!(
						path = %local_path.display(),
						"Skipping non-empty local directory selected for pull deletion"
					);
					continue;
				}
				backup_local_entry(
					state,
					relative_path,
					local_path,
					backup_root,
					true,
					baseline,
				)
				.await?;
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
	Ok(())
}

/// Creates selected remote directories and records newly created paths for rollback.
async fn create_pull_directories(
	project_root: &Path,
	actions: &PullActions,
	state: &mut PullCommitState,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	for relative_path in &actions.directory_creations {
		let local_path = checked_local_path(project_root, relative_path)?;
		let existed = async_fs::symlink_metadata(&local_path).await.is_ok();
		let was_backed_up = state
			.backups
			.iter()
			.any(|backup| backup.original_path == local_path);
		if baseline.is_some_and(|snapshot| !snapshot.entries.contains_key(relative_path))
			&& existed
			&& !was_backed_up
		{
			bail!(
				"Local entry appeared immediately before pull commit; refusing to overwrite it: {}",
				local_path.display()
			);
		}
		ensure_local_directory_recording(
			project_root,
			relative_path,
			&mut state.created_directories,
		)
		.await?;
	}
	Ok(())
}

/// Installs all verified staged downloads into their final paths.
async fn install_staged_downloads(
	project_root: &Path,
	staged_downloads: &[StagedDownload],
	state: &mut PullCommitState,
) -> Result<()> {
	for staged in staged_downloads {
		let local_path = ensure_local_file_parent_recording(
			project_root,
			&staged.relative_path,
			&mut state.created_directories,
		)
		.await?;
		if async_fs::symlink_metadata(&local_path).await.is_ok() {
			bail!(
				"Refusing to install a downloaded file over an unexpected local entry: {}",
				local_path.display()
			);
		}
		let installed = PullInstalledFile {
			path: local_path.clone(),
			fingerprint: staged.fingerprint.clone(),
			#[cfg(unix)]
			rollback_file: staged
				.rollback_file
				.as_ref()
				.wrap_err("Staged pull file is missing its rollback handle")?
				.try_clone()
				.wrap_err("Failed to retain staged pull file for rollback")?,
		};
		async_fs::rename(&staged.staged_path, &local_path)
			.await
			.wrap_err_with(|| {
				format!("Failed to commit downloaded file: {}", local_path.display())
			})?;
		state.installed_files.push(installed);
	}
	Ok(())
}

/// Verifies that an installed file still names the inode and bytes staged by this pull.
#[cfg(unix)]
#[expect(
	clippy::filetype_is_file,
	reason = "rollback verification must reject every entry type except regular files"
)]
fn installed_pull_file_matches(installed: &PullInstalledFile) -> Result<bool> {
	let path_metadata = match fs::symlink_metadata(&installed.path) {
		Ok(metadata) => metadata,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(error) => {
			return Err(error).wrap_err_with(|| {
				format!(
					"Failed to inspect installed pull file: {}",
					installed.path.display()
				)
			});
		}
	};
	if !path_metadata.file_type().is_file() || path_metadata.file_type().is_symlink() {
		return Ok(false);
	}
	let handle_metadata = installed
		.rollback_file
		.metadata()
		.wrap_err("Failed to inspect retained pull file handle")?;
	if path_metadata.dev() != handle_metadata.dev() || path_metadata.ino() != handle_metadata.ino()
	{
		return Ok(false);
	}
	let actual = LocalSnapshotEntry::File {
		hash: hash_open_file(&installed.rollback_file)?,
		mode: Some(path_metadata.permissions().mode() & 0o777),
	};
	Ok(actual == installed.fingerprint)
}

/// Verifies that an installed file is unchanged before rollback removes it.
#[cfg(not(unix))]
fn installed_pull_file_matches(installed: &PullInstalledFile) -> Result<bool> {
	Ok(fingerprint_local_entry(&installed.path)?.as_ref() == Some(&installed.fingerprint))
}

/// Restores the local tree after a failed pull commit.
async fn rollback_pull_commit(state: &PullCommitState) -> Result<()> {
	for installed in state.installed_files.iter().rev() {
		if !installed_pull_file_matches(installed)? {
			bail!(
				"Refusing to remove a locally modified pull result during rollback: {}",
				installed.path.display()
			);
		}
		match async_fs::remove_file(&installed.path).await {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!(
						"Failed to remove installed file during rollback: {}",
						installed.path.display()
					)
				});
			}
		}
	}
	for created_path in state.created_directories.iter().rev() {
		match async_fs::remove_dir(created_path).await {
			Ok(()) => {}
			Err(error)
				if matches!(
					error.kind(),
					io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
				) => {}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!(
						"Failed to remove created directory during rollback: {}",
						created_path.display()
					)
				});
			}
		}
	}
	for backup in state.backups.iter().rev() {
		if let Some(parent) = backup.original_path.parent() {
			async_fs::create_dir_all(parent).await.wrap_err_with(|| {
				format!(
					"Failed to restore parent directory during rollback: {}",
					parent.display()
				)
			})?;
		}
		if async_fs::symlink_metadata(&backup.original_path)
			.await
			.is_ok()
		{
			bail!(
				"Refusing to overwrite an unexpected local entry during rollback: {}",
				backup.original_path.display()
			);
		}
		async_fs::rename(&backup.backup_path, &backup.original_path)
			.await
			.wrap_err_with(|| {
				format!(
					"Failed to restore local entry during rollback: {}",
					backup.original_path.display()
				)
			})?;
	}
	Ok(())
}

/// Process-termination listeners installed before a pull starts staging results.
struct PullInterruptListener {
	/// Shared cancellation state set when a termination signal arrives.
	receiver: watch::Receiver<bool>,
	/// Background task that owns the platform signal receivers.
	task: JoinHandle<()>,
}

#[cfg(unix)]
#[expect(
	clippy::multiple_inherent_impl,
	reason = "platform-specific signal setup shares platform-neutral cancellation methods"
)]
impl PullInterruptListener {
	/// Installs ordinary Unix termination handlers before pull staging begins.
	#[expect(
		clippy::integer_division_remainder_used,
		reason = "tokio::select macro expansion uses remainder internally"
	)]
	fn new() -> Result<Self> {
		let mut interrupt =
			signal(SignalKind::interrupt()).wrap_err("Failed to install pull interrupt handler")?;
		let mut terminate = signal(SignalKind::terminate())
			.wrap_err("Failed to install pull termination handler")?;
		let mut hangup =
			signal(SignalKind::hangup()).wrap_err("Failed to install pull hangup handler")?;
		let mut quit =
			signal(SignalKind::quit()).wrap_err("Failed to install pull quit handler")?;
		let (sender, receiver) = watch::channel(false);
		let task = tokio::spawn(async move {
			tokio::select! {
				_ = interrupt.recv() => {}
				_ = terminate.recv() => {}
				_ = hangup.recv() => {}
				_ = quit.recv() => {}
			}
			if sender.send(true).is_err() {
				debug!("Pull interrupt receiver was already dropped");
			}
		});
		Ok(Self { receiver, task })
	}
}

#[cfg(not(unix))]
impl PullInterruptListener {
	/// Creates the platform interrupt listener.
	fn new() -> Result<Self> {
		let (sender, receiver) = watch::channel(false);
		let task = tokio::spawn(async move {
			if tokio::signal::ctrl_c().await.is_ok() && sender.send(true).is_err() {
				debug!("Pull interrupt receiver was already dropped");
			}
		});
		Ok(Self { receiver, task })
	}
}

impl PullInterruptListener {
	/// Returns whether a termination signal has already arrived.
	fn interrupted(&self) -> bool {
		*self.receiver.borrow()
	}

	/// Waits until a termination signal arrives.
	async fn recv(&self) {
		if self.interrupted() {
			return;
		}
		let mut receiver = self.receiver.clone();
		while receiver.changed().await.is_ok() {
			if *receiver.borrow() {
				return;
			}
		}
		pending::<()>().await;
	}

	/// Gives queued signals a chance to propagate, then rejects interrupted work.
	async fn checkpoint(&self, phase: &str) -> Result<()> {
		yield_now().await;
		if self.interrupted() {
			bail!("Pull interrupted during {phase}");
		}
		Ok(())
	}
}

impl Drop for PullInterruptListener {
	fn drop(&mut self) {
		self.task.abort();
	}
}

/// Completes one pre-commit pull phase or aborts it when termination is requested.
#[expect(
	clippy::integer_division_remainder_used,
	reason = "tokio::select macro expansion uses remainder internally"
)]
async fn complete_pull_phase<T>(
	interrupts: &PullInterruptListener,
	phase: &str,
	future: impl Future<Output = Result<T>>,
) -> Result<T> {
	tokio::pin!(future);
	tokio::select! {
		biased;
		() = interrupts.recv() => Err(eyre!("Pull interrupted during {phase}")),
		result = &mut future => result,
	}
}

/// Applies a pull mutation plan using same-filesystem backups for rollback.
#[expect(
	clippy::integer_division_remainder_used,
	reason = "tokio::select macro expansion uses remainder internally"
)]
async fn commit_pull_transaction_with_interrupts(
	project_root: &Path,
	staging_root: &Path,
	actions: &PullActions,
	staged_downloads: &[StagedDownload],
	baseline: Option<&LocalSnapshot>,
	interrupts: &PullInterruptListener,
) -> Result<(usize, usize)> {
	let backup_root = staging_root.join("backups");
	if let Err(error) = create_private_local_directory(&backup_root) {
		let error = eyre!(error).wrap_err("Failed to create pull backup directory");
		if let Err(cleanup_error) = remove_pull_staging_directory(staging_root).await {
			return Err(error).wrap_err(format!(
				"Pull transaction setup failed and cleanup also failed: {cleanup_error}. Partial data remains at {}",
				staging_root.display()
			));
		}
		return Err(error);
	}

	let mut state = PullCommitState::default();
	let (commit_result, interrupted) = {
		let commit = async {
			backup_pull_file_targets(project_root, &backup_root, actions, &mut state, baseline)
				.await?;
			backup_pull_directory_targets(
				project_root,
				&backup_root,
				actions,
				&mut state,
				baseline,
			)
			.await?;
			create_pull_directories(project_root, actions, &mut state, baseline).await?;
			install_staged_downloads(project_root, staged_downloads, &mut state).await?;
			ensure_pull_backups_unchanged(&state, baseline).await
		};
		tokio::pin!(commit);
		tokio::select! {
			biased;
			() = interrupts.recv() => (commit.as_mut().await, true),
			result = &mut commit => (result, false),
		}
	};
	let commit_result = if interrupted {
		match commit_result {
			Ok(()) => Err(eyre!("Pull interrupted during local commit")),
			Err(error) => Err(error.wrap_err("Pull interrupted during local commit")),
		}
	} else {
		commit_result
	};

	if let Err(commit_error) = commit_result {
		if let Err(rollback_error) = rollback_pull_commit(&state).await {
			return Err(commit_error).wrap_err(format!(
				"Pull commit failed and rollback also failed: {rollback_error}. Recovery data remains at {}",
				staging_root.display()
			));
		}
		if let Err(cleanup_error) = remove_pull_staging_directory(staging_root).await {
			return Err(commit_error).wrap_err(format!(
				"Pull commit failed and local changes were rolled back, but transaction cleanup also failed: {cleanup_error}. Partial data remains at {}",
				staging_root.display()
			));
		}
		return Err(commit_error).wrap_err("Pull commit failed; local changes were rolled back");
	}

	let deleted = state
		.backups
		.iter()
		.filter(|backup| backup.counts_as_deletion)
		.count();
	Ok((deleted, state.installed_files.len()))
}

/// Applies a pull transaction with a fresh interrupt listener in unit tests.
#[cfg(test)]
async fn commit_pull_transaction(
	project_root: &Path,
	staging_root: &Path,
	actions: &PullActions,
	staged_downloads: &[StagedDownload],
	baseline: Option<&LocalSnapshot>,
) -> Result<(usize, usize)> {
	let interrupts = PullInterruptListener::new()?;
	commit_pull_transaction_with_interrupts(
		project_root,
		staging_root,
		actions,
		staged_downloads,
		baseline,
		&interrupts,
	)
	.await
}

/// Executes pull actions by deleting local paths, creating local directories, and downloading files.
#[expect(
	clippy::too_many_lines,
	reason = "pull staging, precondition verification, commit, and cleanup form one transaction"
)]
async fn apply_pull_actions(
	context: &SyncExecution<'_>,
	actions: PullActions,
	expected_remote_state: &RemoteState,
	baseline: Option<&LocalSnapshot>,
	stats: &mut Stats,
) -> Result<()> {
	let SyncExecution {
		client,
		config,
		project_root,
		remote_dir,
		spinner,
		..
	} = context;
	let interrupts = context
		.interrupts
		.wrap_err("Pull interrupt listener was not initialized")?;
	interrupts.checkpoint("pull planning").await?;
	if actions.file_deletions.is_empty()
		&& actions.directory_creations.is_empty()
		&& actions.directory_deletions.is_empty()
		&& actions.downloads.is_empty()
	{
		return Ok(());
	}

	let staging_root = create_pull_staging_directory(project_root)?;
	let downloads_root = staging_root.join("downloads");
	if let Err(error) = create_private_local_directory(&downloads_root) {
		let error = eyre!(error).wrap_err("Failed to create pull downloads directory");
		if let Err(cleanup_error) = remove_pull_staging_directory(&staging_root).await {
			return Err(error).wrap_err(format!(
				"Pull transaction setup failed and cleanup also failed: {cleanup_error}. Partial data remains at {}",
				staging_root.display()
			));
		}
		return Err(error);
	}
	let total_to_download = actions.downloads.len();
	let stage_result = complete_pull_phase(interrupts, "download staging", async {
		if actions.downloads.is_empty() {
			return Ok::<_, color_eyre::Report>(Vec::new());
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
		let mut staged_downloads = Vec::with_capacity(total_to_download);
		for (index, download) in actions.downloads.iter().enumerate() {
			if let Some(spinner) = *spinner {
				spinner.set_message(format!(
					"Downloading files... ({}/{total_to_download})",
					index.saturating_add(1)
				));
			}

			let remote_path = format!("{remote_dir}/{}", download.path);
			let mut staged = stage_download(
				&sftp,
				&remote_path,
				&downloads_root,
				download,
				config.ssh.umask.as_u32(),
			)
			.await?;
			preserve_round_trip_file_mode(&mut staged, baseline).await?;
			staged_downloads.push(staged);
		}
		Ok::<_, color_eyre::Report>(staged_downloads)
	})
	.await;
	let staged_downloads = match stage_result {
		Ok(downloads) => downloads,
		Err(error) => {
			if let Err(cleanup_error) = remove_pull_staging_directory(&staging_root).await {
				return Err(error).wrap_err(format!(
					"Pull staging failed and transaction cleanup also failed: {cleanup_error}. Partial data remains at {}",
					staging_root.display()
				));
			}
			return Err(error);
		}
	};

	if let Err(error) = complete_pull_phase(
		interrupts,
		"precondition verification",
		verify_pull_preconditions(context, expected_remote_state, baseline),
	)
	.await
	{
		if let Err(cleanup_error) = remove_pull_staging_directory(&staging_root).await {
			return Err(error).wrap_err(format!(
				"Pull precondition check failed and transaction cleanup also failed: {cleanup_error}. Partial data remains at {}",
				staging_root.display()
			));
		}
		return Err(error);
	}
	let commit_result = commit_pull_transaction_with_interrupts(
		project_root,
		&staging_root,
		&actions,
		&staged_downloads,
		baseline,
		interrupts,
	)
	.await;
	let (deleted, downloaded) = match commit_result {
		Ok(counts) => counts,
		Err(error) => return Err(error),
	};
	stats.deleted = stats.deleted.saturating_add(deleted);
	stats.downloaded = stats.downloaded.saturating_add(downloaded);
	if let Err(error) = remove_pull_staging_directory(&staging_root).await {
		warn!(
			%error,
			path = %staging_root.display(),
			"Pull committed successfully, but private backup cleanup failed; remove the recovery copies manually"
		);
	}

	Ok(())
}

/// Applies the local filesystem side of a pull operation.
#[cfg(test)]
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

/// Revalidates remote and local state immediately before a pull commit.
async fn verify_pull_preconditions(
	context: &SyncExecution<'_>,
	expected_remote_state: &RemoteState,
	baseline: Option<&LocalSnapshot>,
) -> Result<()> {
	// Verification passes never consult the remote cache: they exist to prove
	// the remote content itself, so every file is hashed again.
	let (remote_state, _) = fetch_remote_state(
		context.client,
		context.config,
		context.remote_dir,
		false,
		None,
	)
	.await?;
	let remote_state = filter_remote_state_for_pull(
		remote_state,
		context.project_root,
		&context.config.sync.exclude,
		context.options,
	)?;
	ensure_no_reserved_pull_paths(&remote_state)?;
	ensure_no_remote_symlinks(&remote_state)?;
	if &remote_state != expected_remote_state {
		bail!(
			"Remote project changed while pull data was being prepared; local files were not modified"
		);
	}

	if let Some(baseline) = baseline {
		let actual =
			snapshot_local_project(context.project_root, context.config, context.options).await?;
		ensure_local_snapshot_matches(baseline, &actual, "after the remote command completed")?;
	}
	Ok(())
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

/// Rejects remote entries in the namespace reserved for private pull transactions.
fn ensure_no_reserved_pull_paths(remote_state: &RemoteState) -> Result<()> {
	let mut reserved = remote_state
		.file_hashes
		.keys()
		.chain(remote_state.directories.iter())
		.chain(remote_state.symlinks.iter())
		.filter(|path| {
			path.split('/')
				.next()
				.is_some_and(|component| component.starts_with(LOCAL_PULL_STAGE_PREFIX))
		})
		.cloned()
		.collect::<Vec<_>>();
	reserved.sort_unstable();
	reserved.dedup();
	if reserved.is_empty() {
		return Ok(());
	}
	bail!(
		"Remote project uses Biwa's reserved pull transaction namespace: {}",
		reserved.join(", ")
	)
}

/// Rejects remote entries that would collide with a preserved baseline symlink.
fn ensure_no_round_trip_symlink_collisions(
	remote_state: &RemoteState,
	baseline: &LocalSnapshot,
) -> Result<()> {
	let remote_paths = remote_state
		.file_hashes
		.keys()
		.chain(remote_state.directories.iter());
	let mut collisions = baseline
		.entries
		.iter()
		.filter_map(|(path, entry)| {
			if !matches!(entry, LocalSnapshotEntry::Symlink { .. }) {
				return None;
			}
			let prefix = format!("{path}/");
			remote_paths
				.clone()
				.any(|remote_path| remote_path == path || remote_path.starts_with(&prefix))
				.then(|| path.clone())
		})
		.collect::<Vec<_>>();
	collisions.sort_unstable();
	if collisions.is_empty() {
		return Ok(());
	}
	bail!(
		"Remote results collide with preserved local symlinks: {}",
		collisions.join(", ")
	)
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
	/// Pull termination state shared by staging and commit.
	interrupts: Option<&'a PullInterruptListener>,
}

/// Plans and applies push synchronization.
///
/// Returns the remote paths the push mutated (uploads and deletions), so
/// callers can invalidate stale sync cache entries for them.
async fn execute_push_sync(
	context: &SyncExecution<'_>,
	local_state: &LocalState,
	remote_state: &RemoteState,
	stats: &mut Stats,
) -> Result<Vec<String>> {
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

	let touched_paths = actions
		.uploads
		.iter()
		.map(|path| path.to_string_lossy().into_owned())
		.chain(actions.file_deletions.iter().cloned())
		.collect::<Vec<_>>();
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
	.await?;
	Ok(touched_paths)
}

/// Plans and applies pull synchronization.
///
/// Returns the local paths the pull mutated (downloads and deletions), so
/// callers can invalidate stale sync cache entries for them.
async fn execute_pull_sync(
	context: &SyncExecution<'_>,
	local_state: &LocalState,
	remote_state: RemoteState,
	baseline: Option<&LocalSnapshot>,
	stats: &mut Stats,
) -> Result<Vec<String>> {
	let remote_state = filter_remote_state_for_pull(
		remote_state,
		context.project_root,
		&context.config.sync.exclude,
		context.options,
	)?;
	ensure_no_reserved_pull_paths(&remote_state)?;
	ensure_no_remote_symlinks(&remote_state)?;
	if let Some(baseline) = baseline {
		let actual = snapshot_from_local_state(context.project_root, local_state)?;
		ensure_local_snapshot_matches(baseline, &actual, "while the remote command was running")?;
		ensure_no_round_trip_symlink_collisions(&remote_state, baseline)?;
	}
	ensure_sync_file_limit(
		remote_state.file_hashes.len(),
		context.config.sync.sftp.max_files_to_sync,
	)?;
	let actions = calculate_pull_actions(
		local_state,
		&remote_state,
		context.options,
		baseline.is_some(),
	);
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

	let touched_paths = actions
		.downloads
		.iter()
		.map(|download| download.path.clone())
		.chain(actions.file_deletions.iter().cloned())
		.collect::<Vec<_>>();
	apply_pull_actions(context, actions, &remote_state, baseline, stats).await?;
	Ok(touched_paths)
}

/// Resolves the sync cache directory and key when the cache is enabled.
///
/// Returns `None` when the cache is disabled or the SSH target cannot be
/// resolved; either way synchronization proceeds without a cache.
fn resolve_cache_context(
	config: &Config,
	project_root: &Path,
	remote_dir: &str,
) -> Option<(PathBuf, CacheKey)> {
	if !config.sync.sftp.cache.enabled {
		return None;
	}
	match ResolvedSshTarget::resolve(&config.ssh) {
		Ok(target) => Some((
			sync_cache::resolve_cache_dir(config),
			CacheKey::new(&target, project_root, remote_dir),
		)),
		Err(error) => {
			warn!(%error, "Failed to resolve SSH target for the sync cache; skipping cache");
			None
		}
	}
}

/// The sync cache state resolved for one synchronization run.
#[derive(Debug, Default)]
struct CacheState {
	/// Cache directory and key, when caching is enabled for this run.
	context: Option<(PathBuf, CacheKey)>,
	/// Cached local file hashes to reuse during the local scan.
	local_files: Option<BTreeMap<String, CachedFileState>>,
	/// Cached remote file hashes to reuse during the remote inventory.
	remote_files: Option<BTreeMap<String, CachedRemoteState>>,
	/// Unix seconds of the last recorded full remote hash pass.
	remote_hashed_at: Option<u64>,
}

impl CacheState {
	/// Loads the sync cache for one run.
	///
	/// `--force` recomputes all local and remote state, so it ignores the stored
	/// hashes entirely; the cache is then rebuilt from scratch after a
	/// successful run, which makes `--force` double as a cache reset. Cached
	/// remote hashes are also ignored on the first run after the automatic
	/// revalidation interval elapses.
	fn load(config: &Config, project_root: &Path, remote_dir: &str, force: bool) -> Self {
		let context = resolve_cache_context(config, project_root, remote_dir);
		let cached = if force {
			None
		} else {
			context
				.as_ref()
				.and_then(|(cache_dir, key)| sync_cache::load_cache(cache_dir, key))
		};
		let remote_hashed_at = cached.as_ref().and_then(|cached| cached.remote_hashed_at);
		let revalidate = config.sync.sftp.cache.auto_revalidate
			&& sync_cache::remote_revalidation_due(remote_hashed_at, sync_cache::unix_now());
		if revalidate && cached.is_some() {
			debug!("Revalidating remote sync state; cached remote hashes are ignored for this run");
		}
		let (local_files, remote_files) = cached.map_or((None, None), |cached| {
			(
				Some(cached.local_files),
				(!revalidate).then_some(cached.remote_files),
			)
		});
		Self {
			context,
			local_files,
			remote_files,
			remote_hashed_at,
		}
	}
}

/// Relative paths one synchronization mutated on either side.
#[derive(Debug, Default)]
struct TouchedPaths {
	/// Local paths the operation rewrote or removed.
	local: Vec<String>,
	/// Remote paths the operation rewrote or removed.
	remote: Vec<String>,
}

/// Persists scan results to the sync cache after a fully successful sync.
///
/// Entries for paths the operation itself mutated are dropped so they are
/// re-hashed on the next run. A failed cache write never fails the sync.
///
/// The stored maps are replaced wholesale with this run's entries rather than
/// merged with the previous cache. That keeps invalidation structural —
/// entries for deleted, excluded, or type-changed paths simply vanish — at the
/// cost of re-hashing unscanned files after a narrowly filtered sync.
///
/// The last full remote hash timestamp only advances when this run actually
/// hashed every remote file, so an incremental run never postpones the next
/// automatic revalidation.
fn persist_scan_cache(
	cache_context: Option<(PathBuf, CacheKey)>,
	scan_cache: LocalScanCache,
	remote_scan_cache: RemoteScanCache,
	touched_paths: &TouchedPaths,
	previous_remote_hashed_at: Option<u64>,
) {
	let Some((cache_dir, key)) = cache_context else {
		return;
	};
	let mut local_files = scan_cache.entries;
	for path in &touched_paths.local {
		local_files.remove(path);
	}
	let mut remote_files = remote_scan_cache.entries;
	for path in &touched_paths.remote {
		remote_files.remove(path);
	}
	let remote_hashed_at = if remote_scan_cache.full_hash {
		sync_cache::unix_now()
	} else {
		previous_remote_hashed_at
	};
	let data = SyncCacheData {
		local_files,
		remote_files,
		remote_hashed_at,
	};
	if let Err(error) = sync_cache::save_cache(&cache_dir, &key, data) {
		warn!(%error, "Failed to save sync cache");
	}
}

/// Collects local synchronization state without blocking the async runtime.
async fn collect_local_state_async(
	project_root: &Path,
	config: &Config,
	options: &Options,
	cached_files: Option<BTreeMap<String, CachedFileState>>,
) -> Result<(LocalState, LocalScanCache)> {
	let project_root = project_root.to_path_buf();
	let exclude = config.sync.exclude.clone();
	let options = options.clone();
	spawn_blocking(move || {
		collect_local_state_cached(&project_root, &exclude, &options, cached_files.as_ref())
	})
	.await
	.wrap_err("Failed to join blocking task")?
}

/// Fully resolved inputs shared by push and pull entrypoints.
struct ProjectTransfer<'a> {
	/// Connected SSH client.
	client: &'a Client,
	/// Loaded configuration.
	config: &'a Config,
	/// Local project root.
	project_root: &'a Path,
	/// Resolved remote project directory.
	remote_dir: &'a str,
	/// Direction-neutral transfer options.
	options: &'a Options,
	/// Suppress progress output.
	quiet: bool,
}

/// Synchronizes a project in one explicit direction.
#[expect(
	clippy::too_many_lines,
	reason = "shared push and pull orchestration keeps one resolved transfer lifecycle"
)]
async fn sync_project(
	transfer: ProjectTransfer<'_>,
	direction: Direction,
	baseline: Option<&LocalSnapshot>,
) -> Result<Stats> {
	let ProjectTransfer {
		client,
		config,
		project_root,
		remote_dir,
		options,
		quiet,
	} = transfer;
	if config.sync.engine != SyncEngine::Sftp {
		bail!("Only SFTP sync engine is currently supported");
	}
	let pull_interrupts = if direction == Direction::Pull {
		Some(PullInterruptListener::new()?)
	} else {
		None
	};
	info!(
			project_root = %project_root.display(),
			direction = ?direction,
			force = options.force,
			include_patterns = options.include.len(),
			exclude_patterns = options.exclude.len(),
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

	let cache = CacheState::load(config, project_root, remote_dir, options.force);

	let fetch = fetch_remote_state(
		client,
		config,
		remote_dir,
		direction == Direction::Push,
		cache.remote_files.as_ref(),
	);
	let (remote_state, remote_scan_cache) = if let Some(interrupts) = pull_interrupts.as_ref() {
		complete_pull_phase(interrupts, "remote inventory", fetch).await?
	} else {
		fetch.await?
	};
	debug!(
		remote_dir,
		remote_directories = remote_state.directories.len(),
		remote_files = remote_state.file_hashes.len(),
		remote_symlinks = remote_state.symlinks.len(),
		remote_cache_hits = remote_scan_cache.hits,
		remote_hashed = remote_scan_cache.hashed,
		remote_full_hash = remote_scan_cache.full_hash,
		"Fetched remote sync state"
	);
	if direction == Direction::Pull {
		ensure_local_pull_root(project_root).await?;
	}

	let collect = collect_local_state_async(project_root, config, options, cache.local_files);
	let (local_state, scan_cache) = if let Some(interrupts) = pull_interrupts.as_ref() {
		complete_pull_phase(interrupts, "local inventory", collect).await?
	} else {
		collect.await?
	};
	info!(
		local_directories = local_state.directories.len(),
		local_files = local_state.files.len(),
		local_symlinks = local_state.symlinks.len(),
		cache_hits = scan_cache.hits,
		"Collected local sync state"
	);
	if direction == Direction::Push {
		ensure_sync_file_limit(local_state.files.len(), config.sync.sftp.max_files_to_sync)?;
	}

	let mut stats = Stats::default();
	let touched_paths = {
		let execution = SyncExecution {
			client,
			config,
			project_root,
			remote_dir,
			options,
			spinner: spinner.as_ref(),
			interrupts: pull_interrupts.as_ref(),
		};
		match direction {
			Direction::Push => TouchedPaths {
				// Push never mutates local files, so every scanned entry stays valid.
				local: Vec::new(),
				remote: execute_push_sync(&execution, &local_state, &remote_state, &mut stats)
					.await?,
			},
			Direction::Pull => TouchedPaths {
				local: execute_pull_sync(
					&execution,
					&local_state,
					remote_state,
					baseline,
					&mut stats,
				)
				.await?,
				// Pull never mutates the remote directory.
				remote: Vec::new(),
			},
		}
	};
	// A successful pull commit is the cancellation boundary: reporting an interrupt
	// after backups are removed would falsely describe durable local changes as failed.
	drop(pull_interrupts);

	if let Some(s) = spinner {
		s.finish_and_clear();
	}
	info!("Sync completed: {:?}", stats);

	persist_scan_cache(
		cache.context,
		scan_cache,
		remote_scan_cache,
		&touched_paths,
		cache.remote_hashed_at,
	);
	if !quiet {
		match direction {
			Direction::Push => {
				eprintln!(
					"{} Push completed: {} uploaded, {} deleted, {} unchanged",
					style("✓").green().bold(),
					stats.uploaded,
					stats.deleted,
					stats.unchanged
				);
			}
			Direction::Pull => {
				eprintln!(
					"{} Pull completed: {} downloaded, {} deleted, {} unchanged",
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

/// Pushes the selected local project state to one resolved remote directory.
pub async fn push_project(
	client: &Client,
	config: &Config,
	project_root: &Path,
	remote_dir: &str,
	options: &Options,
	quiet: bool,
) -> Result<Stats> {
	sync_project(
		ProjectTransfer {
			client,
			config,
			project_root,
			remote_dir,
			options,
			quiet,
		},
		Direction::Push,
		None,
	)
	.await
}

/// Pulls one resolved remote directory into the selected local project root.
///
/// When `baseline` is present, local drift is rejected and pre-existing local
/// symlinks and file modes are preserved for safe round-trip operation.
pub async fn pull_project(
	client: &Client,
	config: &Config,
	project_root: &Path,
	remote_dir: &str,
	options: &Options,
	baseline: Option<&LocalSnapshot>,
	quiet: bool,
) -> Result<Stats> {
	sync_project(
		ProjectTransfer {
			client,
			config,
			project_root,
			remote_dir,
			options,
			quiet,
		},
		Direction::Pull,
		baseline,
	)
	.await
}

/// Section of the NUL-framed remote inventory protocol.
#[derive(Clone, Copy)]
enum RemoteInventorySection {
	/// Remote clock in Unix seconds.
	Clock,
	/// Directory paths.
	Directories,
	/// Symlink paths.
	Symlinks,
	/// File sizes, modification times, and paths.
	FileMetadata,
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

/// Parses one `sha256sum` record into its digest and validated path.
fn parse_remote_hash_record(record: &str) -> Result<(String, String)> {
	let (hash, raw_path) = record
		.split_once("  ")
		.wrap_err("Malformed remote file hash record")?;
	if !is_sha256_hash(hash) {
		bail!("Malformed SHA-256 digest in remote inventory");
	}
	Ok((parse_inventory_path(raw_path)?, hash.to_owned()))
}

/// Parses the NUL-framed output of a targeted remote hashing command.
fn parse_remote_hashes(output: &str) -> Result<HashMap<String, String>> {
	output
		.split_terminator('\0')
		.map(parse_remote_hash_record)
		.collect()
}

/// Parses the strict NUL-framed remote inventory.
fn parse_remote_inventory(output: &str) -> Result<RemoteInventory> {
	let mut inventory = RemoteInventory::default();
	let mut section = None;
	let mut found_sections = HashSet::new();

	for record in output.split_terminator('\0') {
		let marker = match record {
			REMOTE_CLOCK_MARKER => Some(RemoteInventorySection::Clock),
			REMOTE_DIRECTORY_MARKER => Some(RemoteInventorySection::Directories),
			REMOTE_SYMLINK_MARKER => Some(RemoteInventorySection::Symlinks),
			REMOTE_FILE_META_MARKER => Some(RemoteInventorySection::FileMetadata),
			REMOTE_FILE_MARKER => Some(RemoteInventorySection::Files),
			_ => None,
		};
		if let Some(marker) = marker {
			section = Some(marker);
			found_sections.insert(record);
			continue;
		}

		match section.wrap_err("Remote inventory entry appeared before its section marker")? {
			RemoteInventorySection::Clock => {
				inventory.clock = record
					.parse::<i64>()
					.wrap_err("Malformed remote clock in remote inventory")?;
			}
			RemoteInventorySection::Directories => {
				inventory.directories.insert(parse_inventory_path(record)?);
			}
			RemoteInventorySection::Symlinks => {
				inventory.symlinks.insert(parse_inventory_path(record)?);
			}
			RemoteInventorySection::FileMetadata => {
				let malformed = || format!("Malformed remote file metadata record: {record:?}");
				let (size, rest) = record.split_once(' ').wrap_err_with(malformed)?;
				let (mtime, rest) = rest.split_once(' ').wrap_err_with(malformed)?;
				let (ctime, raw_path) = rest.split_once(' ').wrap_err_with(malformed)?;
				let path = parse_inventory_path(raw_path)?;
				// Metadata that cannot key a cache entry is kept as an unusable
				// fingerprint rather than rejected: the file is still part of the
				// remote state, it is simply hashed on every run.
				let fingerprint = RemoteFingerprint::parse(size, mtime, ctime);
				if fingerprint.is_none() {
					debug!(
						path,
						size,
						mtime,
						ctime,
						"Remote file metadata cannot key a cached hash; the file will always be re-hashed"
					);
				}
				inventory.metadata.insert(path, fingerprint);
			}
			RemoteInventorySection::Files => {
				let (path, hash) = parse_remote_hash_record(record)?;
				inventory.hashes.insert(path, hash);
			}
		}
	}

	for marker in [
		REMOTE_CLOCK_MARKER,
		REMOTE_DIRECTORY_MARKER,
		REMOTE_SYMLINK_MARKER,
		REMOTE_FILE_META_MARKER,
		REMOTE_FILE_MARKER,
	] {
		if !found_sections.contains(marker) {
			bail!("Remote inventory is missing section marker: {marker}");
		}
	}

	Ok(inventory)
}

#[cfg(test)]
mod tests {
	use super::*;
	use core::time::Duration;
	use pretty_assertions::{assert_eq, assert_ne};
	use std::fs;
	use std::time::SystemTime;
	use tempfile::tempdir;

	/// Builds a download plan entry for planner assertions.
	fn download(path: &str, expected_hash: &str) -> Download {
		Download {
			path: path.to_owned(),
			expected_hash: expected_hash.to_owned(),
		}
	}

	/// Builds a staged download with its verified fingerprint and rollback handle.
	fn staged_download(relative_path: &str, staged_path: PathBuf) -> StagedDownload {
		let fingerprint = fingerprint_local_entry(&staged_path).unwrap().unwrap();
		StagedDownload {
			relative_path: relative_path.to_owned(),
			#[cfg(unix)]
			rollback_file: Some(File::open(&staged_path).unwrap()),
			staged_path,
			fingerprint,
		}
	}

	/// Builds a deliberately missing staged download for commit-failure tests.
	fn missing_staged_download(relative_path: &str, staged_path: PathBuf) -> StagedDownload {
		StagedDownload {
			relative_path: relative_path.to_owned(),
			staged_path,
			fingerprint: LocalSnapshotEntry::File {
				hash: String::new(),
				mode: None,
			},
			#[cfg(unix)]
			rollback_file: None,
		}
	}

	#[tokio::test]
	async fn pull_interrupt_checkpoint_rejects_a_pretriggered_signal() {
		let (sender, receiver) = watch::channel(false);
		sender.send(true).unwrap();
		let listener = PullInterruptListener {
			receiver,
			task: tokio::spawn(async { pending::<()>().await }),
		};

		let error = listener.checkpoint("pull planning").await.unwrap_err();

		assert_eq!(error.to_string(), "Pull interrupted during pull planning");
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
	fn collect_local_state_excludes_git_metadata_files_and_directories() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join(".git"), "gitdir: /safe/local\n").unwrap();
		fs::create_dir_all(dir.path().join("nested/.GiT")).unwrap();
		fs::write(dir.path().join("nested/.GiT/config"), "metadata").unwrap();
		fs::write(dir.path().join(".GIT"), "case alias").unwrap();
		fs::write(dir.path().join("nested/kept.txt"), "kept").unwrap();

		let state = collect_local_state(dir.path(), &[], &Options::default()).unwrap();
		let files = state
			.files
			.iter()
			.map(|file| file.path.to_string_lossy().into_owned())
			.collect::<HashSet<_>>();
		assert!(!files.contains(".git"));
		assert!(!files.contains(".GIT"));
		assert!(!files.contains("nested/.GiT/config"));
		assert!(files.contains("nested/kept.txt"));
		assert!(!state.directories.contains("nested/.GiT"));
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

	/// Moves a file's modification time far enough into the past to be cacheable.
	fn backdate_mtime(path: &Path) {
		let file = File::options().append(true).open(path).unwrap();
		let mtime = SystemTime::now()
			.checked_sub(Duration::from_secs(60))
			.unwrap();
		file.set_modified(mtime).unwrap();
	}

	/// Captures the cacheable fingerprint of a local file.
	fn capture_fingerprint(path: &Path) -> FileFingerprint {
		FileFingerprint::capture(&fs::symlink_metadata(path).unwrap()).unwrap()
	}

	#[test]
	fn collect_local_state_cached_reuses_hash_for_matching_fingerprint() {
		let dir = tempdir().unwrap();
		let path = dir.path().join("cached.txt");
		fs::write(&path, "content").unwrap();
		backdate_mtime(&path);
		let fingerprint = capture_fingerprint(&path);
		let cached = BTreeMap::from([(
			"cached.txt".to_owned(),
			CachedFileState {
				fingerprint,
				hash: "cached-hash".to_owned(),
			},
		)]);

		let (state, scan_cache) =
			collect_local_state_cached(dir.path(), &[], &Options::default(), Some(&cached))
				.unwrap();

		// The poisoned hash proves the file was not re-read from disk.
		assert_eq!(state.files.first().unwrap().hash, "cached-hash");
		assert_eq!(scan_cache.hits, 1);
		assert_eq!(
			scan_cache.entries.get("cached.txt").unwrap().hash,
			"cached-hash"
		);
	}

	#[test]
	fn collect_local_state_cached_rehashes_on_fingerprint_mismatch() {
		let dir = tempdir().unwrap();
		let path = dir.path().join("changed.txt");
		fs::write(&path, "content").unwrap();
		backdate_mtime(&path);
		let fingerprint = capture_fingerprint(&path);
		let mismatched_fingerprint = FileFingerprint {
			size: fingerprint.size.saturating_add(1),
			..fingerprint
		};
		let cached = BTreeMap::from([(
			"changed.txt".to_owned(),
			CachedFileState {
				fingerprint: mismatched_fingerprint,
				hash: "stale-hash".to_owned(),
			},
		)]);

		let (state, scan_cache) =
			collect_local_state_cached(dir.path(), &[], &Options::default(), Some(&cached))
				.unwrap();

		let expected_hash = hash_file(&path).unwrap();
		assert_eq!(state.files.first().unwrap().hash, expected_hash);
		assert_eq!(scan_cache.hits, 0);
		assert_eq!(
			scan_cache.entries.get("changed.txt").unwrap().hash,
			expected_hash
		);
	}

	#[test]
	fn collect_local_state_cached_skips_caching_recently_modified_files() {
		let dir = tempdir().unwrap();
		let path = dir.path().join("fresh.txt");
		fs::write(&path, "content").unwrap();

		let (state, scan_cache) =
			collect_local_state_cached(dir.path(), &[], &Options::default(), None).unwrap();

		// A just-written file is hashed but excluded from the persisted cache
		// because its fingerprint could be rewritten without changing.
		assert_eq!(state.files.first().unwrap().hash, hash_file(&path).unwrap());
		assert_eq!(scan_cache.hits, 0);
		assert!(scan_cache.entries.is_empty());
	}

	/// Builds one scan cache entry with an arbitrary fingerprint.
	fn scan_cache_entry(hash: &str) -> CachedFileState {
		CachedFileState {
			fingerprint: FileFingerprint {
				size: 5,
				mtime_secs: 1_700_000_000,
				mtime_nanos: 123,
				ctime_secs: 1_700_000_000,
				ctime_nanos: 456,
				inode: 42,
			},
			hash: hash.to_owned(),
		}
	}

	#[test]
	fn persist_scan_cache_drops_touched_paths() {
		let dir = tempdir().unwrap();
		let key = CacheKey {
			host: "cse.unsw.edu.au".to_owned(),
			port: 22,
			user: "z5555555".to_owned(),
			sync_root: PathBuf::from("/home/user/project"),
			remote_dir: "~/.cache/biwa/projects/project-abc".to_owned(),
		};
		let scan_cache = LocalScanCache {
			entries: BTreeMap::from([
				("kept.txt".to_owned(), scan_cache_entry("kept-hash")),
				("pulled.txt".to_owned(), scan_cache_entry("stale-hash")),
				("deleted.txt".to_owned(), scan_cache_entry("gone-hash")),
			]),
			hits: 0,
		};
		let remote_scan_cache = RemoteScanCache {
			entries: BTreeMap::from([
				("kept.txt".to_owned(), remote_scan_cache_entry("kept-hash")),
				(
					"uploaded.txt".to_owned(),
					remote_scan_cache_entry("stale-hash"),
				),
			]),
			hits: 0,
			hashed: 0,
			// An incremental run must not postpone the next revalidation.
			full_hash: false,
		};

		persist_scan_cache(
			Some((dir.path().to_path_buf(), key.clone())),
			scan_cache,
			remote_scan_cache,
			&TouchedPaths {
				local: vec!["pulled.txt".to_owned(), "deleted.txt".to_owned()],
				remote: vec!["uploaded.txt".to_owned()],
			},
			Some(1_700_000_000),
		);

		let saved = sync_cache::load_cache(dir.path(), &key).unwrap();
		assert_eq!(
			saved.local_files.keys().cloned().collect::<Vec<_>>(),
			vec!["kept.txt".to_owned()]
		);
		assert_eq!(
			saved.remote_files.keys().cloned().collect::<Vec<_>>(),
			vec!["kept.txt".to_owned()]
		);
		assert_eq!(saved.remote_hashed_at, Some(1_700_000_000));
	}

	#[test]
	fn persist_scan_cache_records_a_full_remote_hash_pass() {
		let dir = tempdir().unwrap();
		let key = CacheKey {
			host: "cse.unsw.edu.au".to_owned(),
			port: 22,
			user: "z5555555".to_owned(),
			sync_root: PathBuf::from("/home/user/project"),
			remote_dir: "~/.cache/biwa/projects/project-abc".to_owned(),
		};

		persist_scan_cache(
			Some((dir.path().to_path_buf(), key.clone())),
			LocalScanCache::default(),
			RemoteScanCache {
				full_hash: true,
				..RemoteScanCache::default()
			},
			&TouchedPaths::default(),
			Some(1_700_000_000),
		);

		let saved = sync_cache::load_cache(dir.path(), &key).unwrap();
		assert!(saved.remote_hashed_at > Some(1_700_000_000));
	}

	/// Builds a remote fingerprint with settled timestamps.
	fn remote_fingerprint(size: u64) -> RemoteFingerprint {
		RemoteFingerprint::parse(&size.to_string(), "1700000000.5", "1700000000.5").unwrap()
	}

	/// Builds one remote scan cache entry with a settled fingerprint.
	fn remote_scan_cache_entry(hash: &str) -> CachedRemoteState {
		CachedRemoteState {
			fingerprint: remote_fingerprint(5),
			hash: hash.to_owned(),
		}
	}

	/// Remote clock far enough past the fixtures' modification times to settle them.
	const TEST_REMOTE_CLOCK: i64 = 1_700_000_100;

	/// Builds one NUL-framed remote inventory from its sections.
	fn remote_inventory_output(
		directories: &[&str],
		symlinks: &[&str],
		metadata: &[&str],
		hashes: &[&str],
	) -> String {
		let section = |marker: &str, records: &[&str]| {
			records
				.iter()
				.fold(format!("{marker}\0"), |mut output, record| {
					output.push_str(record);
					output.push('\0');
					output
				})
		};
		format!(
			"{}{}{}{}{}",
			section(REMOTE_CLOCK_MARKER, &[&TEST_REMOTE_CLOCK.to_string()]),
			section(REMOTE_DIRECTORY_MARKER, directories),
			section(REMOTE_SYMLINK_MARKER, symlinks),
			section(REMOTE_FILE_META_MARKER, metadata),
			section(REMOTE_FILE_MARKER, hashes),
		)
	}

	#[test]
	fn parse_remote_inventory_rejects_traversal() {
		let hash = "a".repeat(64);
		let output =
			remote_inventory_output(&[], &[], &[], &[&format!("{hash}  ./../invalid/path.txt")]);
		let error = parse_remote_inventory(&output).unwrap_err();
		assert!(error.to_string().contains("unsafe path"), "error: {error}");
	}

	#[test]
	fn parse_remote_inventory_rejects_absolute_paths() {
		let output = remote_inventory_output(&["/etc"], &[], &[], &[]);
		let error = parse_remote_inventory(&output).unwrap_err();
		assert!(error.to_string().contains("unsafe path"), "error: {error}");
	}

	#[test]
	fn parse_remote_inventory_collects_nul_framed_entries() {
		let hash = "a".repeat(64);
		let output = remote_inventory_output(
			&["./empty", "./nested/child"],
			&["./link"],
			&["12 1700000000.5 1700000000.5 ./nested/file\nname.txt"],
			&[&format!("{hash}  ./nested/file\nname.txt")],
		);

		let inventory = parse_remote_inventory(&output).unwrap();

		assert_eq!(inventory.clock, TEST_REMOTE_CLOCK);
		assert!(inventory.directories.contains("empty"));
		assert!(inventory.directories.contains("nested/child"));
		assert!(inventory.symlinks.contains("link"));
		// Paths may contain spaces and newlines, so only the first three fields
		// of a metadata record are split off.
		assert_eq!(
			inventory
				.metadata
				.get("nested/file\nname.txt")
				.unwrap()
				.clone(),
			Some(remote_fingerprint(12))
		);
		assert_eq!(
			inventory.hashes.get("nested/file\nname.txt").unwrap(),
			&hash
		);
	}

	#[test]
	fn parse_remote_inventory_rejects_malformed_or_incomplete_inventory() {
		let malformed = remote_inventory_output(&[], &[], &[], &["not-a-hash  ./file.txt"]);
		let malformed_error = parse_remote_inventory(&malformed).unwrap_err();
		assert!(
			malformed_error.to_string().contains("Malformed SHA-256"),
			"error: {malformed_error}"
		);
		let malformed_metadata = remote_inventory_output(&[], &[], &["12 ./file.txt"], &[]);
		let metadata_error = parse_remote_inventory(&malformed_metadata).unwrap_err();
		assert!(
			metadata_error
				.to_string()
				.contains("Malformed remote file metadata"),
			"error: {metadata_error}"
		);
		// The offending record is named so the failure is actionable.
		assert!(
			metadata_error.to_string().contains("./file.txt"),
			"error: {metadata_error}"
		);
		let incomplete_error = parse_remote_inventory(REMOTE_DIRECTORY_MARKER).unwrap_err();
		assert!(
			incomplete_error
				.to_string()
				.contains("missing section marker"),
			"error: {incomplete_error}"
		);
	}

	#[test]
	fn parse_remote_inventory_degrades_unusable_metadata_instead_of_failing() {
		let hash = "a".repeat(64);
		// GNU find prints a leading sign for a pre-epoch modification time. Such
		// a file must still appear in the remote state.
		let output = remote_inventory_output(
			&[],
			&[],
			&["3 -315576000.0000000000 1700000000.5 ./old.txt"],
			&[&format!("{hash}  ./old.txt")],
		);

		let inventory = parse_remote_inventory(&output).unwrap();

		assert_eq!(inventory.metadata.get("old.txt").unwrap().clone(), None);

		// It syncs normally and is simply never cached, so it is hashed again on
		// every run instead of silently looking remotely deleted.
		let (state, scan) = build_remote_state(inventory, None, InventoryMode::Full);
		assert_eq!(state.file_hashes.get("old.txt").unwrap(), &hash);
		assert!(scan.entries.is_empty());
	}

	#[test]
	fn pending_hash_paths_includes_files_without_a_usable_fingerprint() {
		let inventory = RemoteInventory {
			clock: TEST_REMOTE_CLOCK,
			metadata: BTreeMap::from([("old.txt".to_owned(), None)]),
			..RemoteInventory::default()
		};
		let cached = BTreeMap::from([("old.txt".to_owned(), remote_scan_cache_entry("cached"))]);

		// A cached entry can never be matched without a fingerprint to compare.
		assert_eq!(
			pending_hash_paths(&inventory, Some(&cached)),
			vec!["old.txt".to_owned()]
		);
	}

	#[test]
	fn build_remote_state_script_creates_only_for_push() {
		let config = Config::default();

		let push_script =
			build_remote_state_script(&config, "~/project", true, InventoryMode::Full);
		assert!(push_script.contains("mkdir -p -- \"$HOME\"/project"));
		assert!(push_script.contains("find . -mindepth 1 -iname .git -prune"));
		assert!(push_script.contains("find . -mindepth 1 -iname .git -prune -o -type d -print0"));
		assert!(push_script.contains("find . -iname .git -prune -o -type l -print0"));
		assert!(push_script.contains("find . -iname .git -prune -o -type f"));
		assert!(push_script.contains("-print0"));
		assert!(push_script.contains("sha256sum -z"));
		assert!(push_script.contains(REMOTE_CLOCK_MARKER));
		assert!(push_script.contains(REMOTE_DIRECTORY_MARKER));
		assert!(push_script.contains(REMOTE_SYMLINK_MARKER));
		assert!(push_script.contains(REMOTE_FILE_META_MARKER));
		assert!(push_script.contains(REMOTE_FILE_MARKER));

		let pull_script =
			build_remote_state_script(&config, "~/project", false, InventoryMode::Full);
		assert!(!pull_script.contains("mkdir -p --"));
		assert!(!pull_script.contains("-exec chmod"));
		assert!(!pull_script.contains("|| true"));
		assert!(pull_script.contains("remote directory does not exist"));
		assert!(pull_script.contains("remote directory is not a directory"));
	}

	#[test]
	fn build_remote_state_script_skips_hashing_for_an_incremental_inventory() {
		let config = Config::default();

		let script =
			build_remote_state_script(&config, "~/project", true, InventoryMode::Incremental);

		// Metadata is still listed, but nothing is hashed remotely.
		assert!(script.contains("-printf '%s %T@ %C@ %p\\0'"));
		assert!(!script.contains("sha256sum"));
		assert!(script.contains(REMOTE_FILE_MARKER));
		// The metadata listing must precede the hash marker so a file rewritten
		// between the passes is recorded with a stale fingerprint, not a stale hash.
		assert!(
			script.find(REMOTE_FILE_META_MARKER) < script.find(REMOTE_FILE_MARKER),
			"script: {script}"
		);
	}

	#[test]
	fn build_remote_hash_command_quotes_paths_and_bounds_its_length() {
		let command = build_remote_hash_command(
			"~/project",
			&["weird name.txt".to_owned(), "-dash.txt".to_owned()],
		)
		.unwrap();

		assert!(command.starts_with("cd -- \"$HOME\"/project && printf '%s\\0'"));
		assert!(command.contains("'./weird name.txt'"));
		// A leading dash is neutralized by the ./ prefix rather than by quoting.
		assert!(command.contains("./-dash.txt"));
		assert!(command.ends_with(" | xargs -0 sha256sum -z --"));

		let many = (0..MAX_REMOTE_HASH_COMMAND_LEN)
			.map(|index| format!("file-{index}.txt"))
			.collect::<Vec<_>>();
		assert_eq!(build_remote_hash_command("~/project", &many), None);

		// The bound covers the finished command, suffix included.
		let mut longest = Vec::new();
		while let Some(command) = build_remote_hash_command("~/project", &longest) {
			assert!(
				command.len() <= MAX_REMOTE_HASH_COMMAND_LEN,
				"command length {}",
				command.len()
			);
			longest.push(format!("file-{}.txt", longest.len()));
		}
	}

	#[test]
	fn pending_hash_paths_lists_only_files_the_cache_cannot_explain() {
		let hash = "a".repeat(64);
		let inventory = RemoteInventory {
			clock: TEST_REMOTE_CLOCK,
			metadata: BTreeMap::from([
				("cached.txt".to_owned(), Some(remote_fingerprint(5))),
				("changed.txt".to_owned(), Some(remote_fingerprint(9))),
				("new.txt".to_owned(), Some(remote_fingerprint(5))),
				("hashed.txt".to_owned(), Some(remote_fingerprint(5))),
			]),
			hashes: HashMap::from([("hashed.txt".to_owned(), hash)]),
			..RemoteInventory::default()
		};
		let cached = BTreeMap::from([
			("cached.txt".to_owned(), remote_scan_cache_entry("cached")),
			("changed.txt".to_owned(), remote_scan_cache_entry("stale")),
		]);

		assert_eq!(
			pending_hash_paths(&inventory, Some(&cached)),
			vec!["changed.txt".to_owned(), "new.txt".to_owned()]
		);
		// Without a cache every metadata entry needs a hash.
		assert_eq!(
			pending_hash_paths(&inventory, None),
			vec![
				"cached.txt".to_owned(),
				"changed.txt".to_owned(),
				"new.txt".to_owned()
			]
		);
	}

	#[test]
	fn build_remote_state_reuses_cached_hashes_for_matching_fingerprints() {
		let hash = "a".repeat(64);
		let inventory = RemoteInventory {
			clock: TEST_REMOTE_CLOCK,
			directories: HashSet::from(["nested".to_owned()]),
			symlinks: HashSet::from(["link".to_owned()]),
			metadata: BTreeMap::from([
				("cached.txt".to_owned(), Some(remote_fingerprint(5))),
				("hashed.txt".to_owned(), Some(remote_fingerprint(9))),
			]),
			hashes: HashMap::from([("hashed.txt".to_owned(), hash.clone())]),
		};
		let cached = BTreeMap::from([(
			"cached.txt".to_owned(),
			remote_scan_cache_entry("cached-hash"),
		)]);

		let (state, scan) =
			build_remote_state(inventory, Some(&cached), InventoryMode::Incremental);

		// The poisoned hash proves the remote file was not hashed again.
		assert_eq!(state.file_hashes.get("cached.txt").unwrap(), "cached-hash");
		assert_eq!(state.file_hashes.get("hashed.txt").unwrap(), &hash);
		assert!(state.directories.contains("nested"));
		assert!(state.symlinks.contains("link"));
		assert_eq!(scan.hits, 1);
		assert_eq!(scan.hashed, 1);
		assert!(!scan.full_hash);
		assert_eq!(scan.entries.len(), 2);
	}

	#[test]
	fn build_remote_state_ignores_a_stale_cached_fingerprint() {
		let hash = "a".repeat(64);
		let inventory = RemoteInventory {
			clock: TEST_REMOTE_CLOCK,
			metadata: BTreeMap::from([("drifted.txt".to_owned(), Some(remote_fingerprint(9)))]),
			hashes: HashMap::from([("drifted.txt".to_owned(), hash.clone())]),
			..RemoteInventory::default()
		};
		// The cached entry records a different size, so it must not be trusted.
		let cached = BTreeMap::from([(
			"drifted.txt".to_owned(),
			remote_scan_cache_entry("stale-hash"),
		)]);

		let (state, scan) =
			build_remote_state(inventory, Some(&cached), InventoryMode::Incremental);

		assert_eq!(state.file_hashes.get("drifted.txt").unwrap(), &hash);
		assert_eq!(scan.hits, 0);
		assert_eq!(scan.entries.get("drifted.txt").unwrap().hash, hash);
	}

	#[test]
	fn build_remote_state_skips_caching_recently_modified_remote_files() {
		let hash = "a".repeat(64);
		let clock = TEST_REMOTE_CLOCK.to_string();
		let fingerprint = RemoteFingerprint::parse("5", &clock, &clock).unwrap();
		let inventory = RemoteInventory {
			clock: TEST_REMOTE_CLOCK,
			metadata: BTreeMap::from([("fresh.txt".to_owned(), Some(fingerprint))]),
			hashes: HashMap::from([("fresh.txt".to_owned(), hash.clone())]),
			..RemoteInventory::default()
		};

		let (state, scan) = build_remote_state(inventory, None, InventoryMode::Full);

		// The file is synchronized normally but stays out of the cache because a
		// rewrite could keep its fingerprint identical.
		assert_eq!(state.file_hashes.get("fresh.txt").unwrap(), &hash);
		assert!(scan.entries.is_empty());
		assert!(scan.full_hash);
	}

	#[test]
	fn build_remote_state_drops_files_that_vanished_before_hashing() {
		let hash = "a".repeat(64);
		let inventory = RemoteInventory {
			clock: TEST_REMOTE_CLOCK,
			metadata: BTreeMap::from([("vanished.txt".to_owned(), Some(remote_fingerprint(5)))]),
			// A file created after the metadata pass has a hash but no fingerprint.
			hashes: HashMap::from([("appeared.txt".to_owned(), hash.clone())]),
			..RemoteInventory::default()
		};

		let (state, scan) = build_remote_state(inventory, None, InventoryMode::Full);

		assert!(!state.file_hashes.contains_key("vanished.txt"));
		assert_eq!(state.file_hashes.get("appeared.txt").unwrap(), &hash);
		assert!(scan.entries.is_empty());
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
	fn calculate_push_actions_replaces_remote_file_with_local_directory() {
		let actions = calculate_push_actions(
			&LocalState {
				files: vec![LocalFile {
					path: PathBuf::from("node/child.txt"),
					hash: "local".to_owned(),
				}],
				directories: HashSet::from(["node".to_owned()]),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::from([("node".to_owned(), "remote".to_owned())]),
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(actions.uploads, vec![PathBuf::from("node/child.txt")]);
		assert_eq!(actions.file_deletions, vec!["node".to_owned()]);
		assert_eq!(actions.directory_creations, vec!["node".to_owned()]);
		assert!(actions.directory_deletions.is_empty());
	}

	#[test]
	fn calculate_push_actions_replaces_remote_directory_with_local_file() {
		let actions = calculate_push_actions(
			&LocalState {
				files: vec![LocalFile {
					path: PathBuf::from("node"),
					hash: "local".to_owned(),
				}],
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::from([("node/child.txt".to_owned(), "remote".to_owned())]),
				directories: HashSet::from(["node".to_owned()]),
				symlinks: HashSet::new(),
			},
			&Options::default(),
		);

		assert_eq!(actions.uploads, vec![PathBuf::from("node")]);
		assert_eq!(actions.file_deletions, vec!["node/child.txt".to_owned()]);
		assert!(actions.directory_creations.is_empty());
		assert_eq!(actions.directory_deletions, vec!["node".to_owned()]);
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
			false,
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
	fn calculate_push_actions_deletes_unsupported_remote_symlinks() {
		let actions = calculate_push_actions(
			&LocalState::default(),
			&RemoteState {
				file_hashes: HashMap::new(),
				directories: HashSet::new(),
				symlinks: HashSet::from(["link".to_owned()]),
			},
			&Options::default(),
		);

		assert_eq!(actions.file_deletions, vec!["link".to_owned()]);
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

		let actions =
			calculate_pull_actions(&local_state, &remote_state, &Options::default(), false);
		assert!(actions.downloads.is_empty());
		assert_eq!(actions.unchanged, 1);

		let actions = calculate_pull_actions(
			&local_state,
			&remote_state,
			&Options {
				force: true,
				..Options::default()
			},
			false,
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
			false,
		);

		assert_eq!(actions.file_deletions, vec!["stale.txt".to_owned()]);
		assert_eq!(actions.directory_deletions, vec!["old".to_owned()]);
		assert_eq!(actions.directory_creations, vec!["empty".to_owned()]);
		assert!(actions.downloads.is_empty());
	}

	#[test]
	fn calculate_pull_actions_replaces_local_file_with_remote_directory() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: vec![LocalFile {
					path: PathBuf::from("node"),
					hash: "local".to_owned(),
				}],
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::from([("node/child.txt".to_owned(), "remote".to_owned())]),
				directories: HashSet::from(["node".to_owned()]),
				symlinks: HashSet::new(),
			},
			&Options::default(),
			false,
		);

		assert_eq!(
			actions.downloads,
			vec![download("node/child.txt", "remote")]
		);
		assert_eq!(actions.file_deletions, vec!["node".to_owned()]);
		assert_eq!(actions.directory_creations, vec!["node".to_owned()]);
		assert!(actions.directory_deletions.is_empty());
	}

	#[test]
	fn calculate_pull_actions_replaces_local_directory_with_remote_file() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: vec![LocalFile {
					path: PathBuf::from("node/child.txt"),
					hash: "local".to_owned(),
				}],
				directories: HashSet::from(["node".to_owned()]),
				symlinks: HashSet::new(),
			},
			&RemoteState {
				file_hashes: HashMap::from([("node".to_owned(), "remote".to_owned())]),
				directories: HashSet::new(),
				symlinks: HashSet::new(),
			},
			&Options::default(),
			false,
		);

		assert_eq!(actions.downloads, vec![download("node", "remote")]);
		assert_eq!(actions.file_deletions, vec!["node/child.txt".to_owned()]);
		assert!(actions.directory_creations.is_empty());
		assert_eq!(actions.directory_deletions, vec!["node".to_owned()]);
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
			false,
		);

		assert_eq!(actions.file_deletions, vec!["link".to_owned()]);
	}

	#[test]
	fn calculate_round_trip_pull_preserves_local_symlinks() {
		let actions = calculate_pull_actions(
			&LocalState {
				files: Vec::new(),
				directories: HashSet::new(),
				symlinks: HashSet::from(["link".to_owned()]),
			},
			&RemoteState::default(),
			&Options::default(),
			true,
		);

		assert!(actions.file_deletions.is_empty());
	}

	#[test]
	fn local_snapshot_reports_content_drift() {
		let expected = LocalSnapshot {
			entries: BTreeMap::from([(
				"file.txt".to_owned(),
				LocalSnapshotEntry::File {
					hash: "before".to_owned(),
					mode: Some(0o644),
				},
			)]),
		};
		let actual = LocalSnapshot {
			entries: BTreeMap::from([(
				"file.txt".to_owned(),
				LocalSnapshotEntry::File {
					hash: "after".to_owned(),
					mode: Some(0o644),
				},
			)]),
		};

		let error = ensure_local_snapshot_unchanged(&expected, &actual, "during test").unwrap_err();
		assert!(error.to_string().contains("file.txt"), "error: {error}");
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
			false,
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

	#[test]
	fn create_pull_staging_directory_uses_private_random_names() {
		let dir = tempdir().unwrap();

		let first = create_pull_staging_directory(dir.path()).unwrap();
		let second = create_pull_staging_directory(dir.path()).unwrap();
		assert_ne!(first, second);
		assert!(
			first
				.file_name()
				.unwrap()
				.to_string_lossy()
				.starts_with(LOCAL_PULL_STAGE_PREFIX)
		);
		assert!(first.is_dir());

		#[cfg(unix)]
		assert_eq!(
			fs::metadata(&first).unwrap().permissions().mode() & 0o777,
			0o700
		);
	}

	#[tokio::test]
	async fn pull_commit_failure_restores_backed_up_files() {
		let dir = tempdir().unwrap();
		let first = dir.path().join("first.txt");
		let second = dir.path().join("second.txt");
		fs::write(&first, "first original").unwrap();
		fs::write(&second, "second original").unwrap();
		let staging_root = dir.path().join(".biwa-pull-stage-test");
		let downloads_root = staging_root.join("downloads");
		fs::create_dir_all(&downloads_root).unwrap();
		let staged_first = downloads_root.join("first.txt");
		fs::write(&staged_first, "first remote").unwrap();
		let staged_downloads = vec![
			staged_download("first.txt", staged_first),
			missing_staged_download("second.txt", downloads_root.join("missing-second.txt")),
		];
		let actions = PullActions {
			downloads: vec![
				download("first.txt", "hash"),
				download("second.txt", "hash"),
			],
			unchanged: 0,
			file_deletions: Vec::new(),
			directory_creations: Vec::new(),
			directory_deletions: Vec::new(),
		};

		let error =
			commit_pull_transaction(dir.path(), &staging_root, &actions, &staged_downloads, None)
				.await
				.unwrap_err();

		assert!(error.to_string().contains("rolled back"), "error: {error}");
		assert_eq!(fs::read_to_string(first).unwrap(), "first original");
		assert_eq!(fs::read_to_string(second).unwrap(), "second original");
		assert!(!staging_root.exists());
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn pull_commit_rolls_back_an_installed_unreadable_file() {
		let dir = tempdir().unwrap();
		let original = dir.path().join("first.txt");
		fs::write(&original, "first original").unwrap();
		let staging_root = dir.path().join(".biwa-pull-stage-test");
		let downloads_root = staging_root.join("downloads");
		fs::create_dir_all(&downloads_root).unwrap();
		let staged_path = downloads_root.join("first.txt");
		fs::write(&staged_path, "first remote").unwrap();
		let mut first_staged = staged_download("first.txt", staged_path.clone());
		fs::set_permissions(&staged_path, fs::Permissions::from_mode(0o000)).unwrap();
		first_staged.fingerprint = LocalSnapshotEntry::File {
			hash: hex::encode(Sha256::digest(b"first remote")),
			mode: Some(0o000),
		};
		let staged_downloads = vec![
			first_staged,
			missing_staged_download("second.txt", downloads_root.join("missing.txt")),
		];
		let actions = PullActions {
			downloads: vec![
				download("first.txt", "hash"),
				download("second.txt", "hash"),
			],
			unchanged: 0,
			file_deletions: Vec::new(),
			directory_creations: Vec::new(),
			directory_deletions: Vec::new(),
		};

		let error =
			commit_pull_transaction(dir.path(), &staging_root, &actions, &staged_downloads, None)
				.await
				.unwrap_err();

		assert!(error.to_string().contains("rolled back"), "error: {error}");
		assert_eq!(fs::read_to_string(original).unwrap(), "first original");
		assert!(!staging_root.exists());
	}

	#[tokio::test]
	async fn pull_commit_failure_removes_every_created_parent_directory() {
		let dir = tempdir().unwrap();
		let staging_root = dir.path().join(".biwa-pull-stage-test");
		let downloads_root = staging_root.join("downloads");
		fs::create_dir_all(&downloads_root).unwrap();
		let actions = PullActions {
			downloads: vec![download("new/parent/result.txt", "hash")],
			unchanged: 0,
			file_deletions: Vec::new(),
			directory_creations: vec!["new/parent".to_owned()],
			directory_deletions: Vec::new(),
		};
		let staged_downloads = vec![missing_staged_download(
			"new/parent/result.txt",
			downloads_root.join("missing.txt"),
		)];

		let error =
			commit_pull_transaction(dir.path(), &staging_root, &actions, &staged_downloads, None)
				.await
				.unwrap_err();

		assert!(error.to_string().contains("rolled back"), "error: {error}");
		assert!(!dir.path().join("new").exists());
		assert!(!staging_root.exists());
	}

	#[tokio::test]
	async fn pull_backup_revalidation_detects_late_file_and_directory_changes() {
		let dir = tempdir().unwrap();
		let backup_root = dir.path().join("backups");
		fs::create_dir_all(&backup_root).unwrap();
		let original_file = dir.path().join("file.txt");
		let original_directory = dir.path().join("empty");
		fs::write(&original_file, "baseline").unwrap();
		fs::create_dir_all(&original_directory).unwrap();
		let baseline = LocalSnapshot {
			entries: BTreeMap::from([
				(
					"file.txt".to_owned(),
					fingerprint_local_entry(&original_file).unwrap().unwrap(),
				),
				(
					"empty".to_owned(),
					fingerprint_local_entry(&original_directory)
						.unwrap()
						.unwrap(),
				),
			]),
		};
		let mut state = PullCommitState::default();
		backup_local_entry(
			&mut state,
			"file.txt",
			original_file,
			&backup_root,
			false,
			Some(&baseline),
		)
		.await
		.unwrap();
		backup_local_entry(
			&mut state,
			"empty",
			original_directory,
			&backup_root,
			true,
			Some(&baseline),
		)
		.await
		.unwrap();

		let file_backup = state.backups.first().unwrap().backup_path.clone();
		let directory_backup = state.backups.last().unwrap().backup_path.clone();
		fs::write(&file_backup, "late edit").unwrap();
		let error = ensure_pull_backups_unchanged(&state, Some(&baseline))
			.await
			.unwrap_err();
		assert!(error.to_string().contains("file.txt"), "error: {error}");

		fs::write(file_backup, "baseline").unwrap();
		fs::write(directory_backup.join("late.txt"), "late").unwrap();
		let error = ensure_pull_backups_unchanged(&state, Some(&baseline))
			.await
			.unwrap_err();
		assert!(error.to_string().contains("empty"), "error: {error}");
	}

	#[tokio::test]
	async fn pull_commit_rejects_late_edit_and_restores_it() {
		let dir = tempdir().unwrap();
		let local_path = dir.path().join("result.txt");
		fs::write(&local_path, "baseline").unwrap();
		let baseline = LocalSnapshot {
			entries: BTreeMap::from([(
				"result.txt".to_owned(),
				fingerprint_local_entry(&local_path).unwrap().unwrap(),
			)]),
		};
		fs::write(&local_path, "late local edit").unwrap();

		let staging_root = dir.path().join(".biwa-pull-stage-test");
		let downloads_root = staging_root.join("downloads");
		fs::create_dir_all(&downloads_root).unwrap();
		let staged_path = downloads_root.join("result.txt");
		fs::write(&staged_path, "remote result").unwrap();
		let actions = PullActions {
			downloads: vec![download("result.txt", "hash")],
			unchanged: 0,
			file_deletions: Vec::new(),
			directory_creations: Vec::new(),
			directory_deletions: Vec::new(),
		};
		let staged_downloads = vec![staged_download("result.txt", staged_path)];

		let error = commit_pull_transaction(
			dir.path(),
			&staging_root,
			&actions,
			&staged_downloads,
			Some(&baseline),
		)
		.await
		.unwrap_err();

		assert!(error.to_string().contains("rolled back"), "error: {error}");
		assert_eq!(fs::read_to_string(local_path).unwrap(), "late local edit");
		assert!(!staging_root.exists());
	}

	#[tokio::test]
	async fn pull_commit_rejects_late_creation_and_restores_it() {
		let dir = tempdir().unwrap();
		let local_path = dir.path().join("result.txt");
		let baseline = LocalSnapshot {
			entries: BTreeMap::new(),
		};
		fs::write(&local_path, "late local creation").unwrap();

		let staging_root = dir.path().join(".biwa-pull-stage-test");
		let downloads_root = staging_root.join("downloads");
		fs::create_dir_all(&downloads_root).unwrap();
		let staged_path = downloads_root.join("result.txt");
		fs::write(&staged_path, "remote result").unwrap();
		let actions = PullActions {
			downloads: vec![download("result.txt", "hash")],
			unchanged: 0,
			file_deletions: Vec::new(),
			directory_creations: Vec::new(),
			directory_deletions: Vec::new(),
		};
		let staged_downloads = vec![staged_download("result.txt", staged_path)];

		let _error = commit_pull_transaction(
			dir.path(),
			&staging_root,
			&actions,
			&staged_downloads,
			Some(&baseline),
		)
		.await
		.unwrap_err();

		assert_eq!(
			fs::read_to_string(local_path).unwrap(),
			"late local creation"
		);
		assert!(!staging_root.exists());
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
	fn filter_remote_state_for_pull_excludes_all_git_metadata_components() {
		let dir = tempdir().unwrap();
		let filtered = filter_remote_state_for_pull(
			RemoteState {
				file_hashes: HashMap::from([
					(".git".to_owned(), "root".to_owned()),
					("nested/.GiT/config".to_owned(), "nested".to_owned()),
					("kept.txt".to_owned(), "kept".to_owned()),
				]),
				directories: HashSet::from(["nested".to_owned(), "nested/.GiT".to_owned()]),
				symlinks: HashSet::from(["other/.GIT/link".to_owned()]),
			},
			dir.path(),
			&[],
			&Options::default(),
		)
		.unwrap();

		assert_eq!(
			filtered.file_hashes,
			HashMap::from([("kept.txt".to_owned(), "kept".to_owned())])
		);
		assert_eq!(filtered.directories, HashSet::from(["nested".to_owned()]));
		assert!(filtered.symlinks.is_empty());
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
	fn project_remote_dir_and_layout_checks_share_normalization() {
		let project = tempdir().unwrap();
		let mut config = Config::default();
		config.sync.remote_root = PathBuf::from("~//.cache/./biwa/projects///");

		let remote_dir = compute_project_remote_dir(&config, project.path()).unwrap();
		assert!(remote_dir.starts_with("~/.cache/biwa/projects/"));
		assert!(!remote_dir.contains("//"));
		assert!(is_default_biwa_remote_dir(
			&remote_dir,
			&config.sync.remote_root,
			&compute_client_host_hash(),
		));
	}

	#[test]
	fn project_remote_dir_and_layout_checks_support_current_remote_directory() {
		let project = tempdir().unwrap();
		let mut config = Config::default();
		config.sync.remote_root = PathBuf::from(".");

		let remote_dir = compute_project_remote_dir(&config, project.path()).unwrap();
		assert!(!remote_dir.contains('/'));
		assert!(is_default_biwa_remote_dir(
			&remote_dir,
			&config.sync.remote_root,
			&compute_client_host_hash(),
		));
	}

	#[test]
	fn project_remote_dir_and_layout_checks_support_filesystem_root() {
		let project = tempdir().unwrap();
		let mut config = Config::default();
		config.sync.remote_root = PathBuf::from("/");

		let remote_dir = compute_project_remote_dir(&config, project.path()).unwrap();
		assert!(remote_dir.starts_with('/'));
		assert_eq!(remote_dir.matches('/').count(), 1);
		assert!(is_default_biwa_remote_dir(
			&remote_dir,
			&config.sync.remote_root,
			&compute_client_host_hash(),
		));
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
		assert_eq!(shell_quote_path("$HOME//foo/bar"), "\"$HOME\"/foo/bar");
		assert_eq!(shell_quote_path("~///foo/bar"), "\"$HOME\"/foo/bar");
	}

	#[test]
	fn resolve_sftp_path() {
		assert_eq!(super::resolve_sftp_path("~/foo/bar"), "foo/bar");
		assert_eq!(super::resolve_sftp_path("$HOME/foo/bar"), "foo/bar");
		assert_eq!(super::resolve_sftp_path("~"), ".");
		assert_eq!(super::resolve_sftp_path("$HOME"), ".");
		assert_eq!(super::resolve_sftp_path("~/"), ".");
		assert_eq!(super::resolve_sftp_path("~//foo/bar"), "foo/bar");
		assert_eq!(super::resolve_sftp_path("$HOME///foo/bar"), "foo/bar");
		assert_eq!(super::resolve_sftp_path("/absolute/path"), "/absolute/path");
		assert_eq!(super::resolve_sftp_path("relative/path"), "relative/path");
	}
}
