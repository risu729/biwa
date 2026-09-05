use crate::Result;
use crate::config::types::Config;
use crate::ssh::target::ResolvedSshTarget;
use alloc::collections::BTreeMap;
use color_eyre::eyre::Context as _;
use core::time::Duration;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

/// Current on-disk sync cache format version.
///
/// Version 2 added the remote inventory cache and version 3 added the remote
/// change time to it. Older files are ignored rather than migrated: the only
/// cost is one full scan, and an explicit version mismatch reports the reason
/// far more clearly than a deserialization failure would.
const CACHE_VERSION: u32 = 3;

/// Subdirectory of the state directory holding sync cache files.
const CACHE_DIR_NAME: &str = "sync_cache";

/// Minimum age a local modification time must have before its hash is cached.
///
/// Filesystems may store modification times at a coarse granularity, so a file
/// modified again shortly after being hashed could keep an identical
/// fingerprint while its content changed. Skipping very recent files keeps the
/// cache trustworthy at the cost of re-hashing them on the next run.
const RACY_MTIME_WINDOW: Duration = Duration::from_secs(RACY_MTIME_WINDOW_SECS);

/// [`RACY_MTIME_WINDOW`] in whole seconds, for remote timestamp comparisons.
///
/// `REMOTE_SETTLE_WINDOW` in `tests/ssh_e2e_sync.rs` mirrors this value so the
/// end-to-end tests wait long enough for a remote fingerprint to be cacheable;
/// update it alongside this constant.
const RACY_MTIME_WINDOW_SECS: u64 = 2;

/// Maximum age of a full remote hash pass before automatic revalidation runs.
///
/// Remote fingerprints have no inode component and rely on timestamps reported
/// by the remote filesystem, so a rewrite is only guaranteed to be visible when
/// the filesystem stamps change times faithfully. Re-hashing the whole remote
/// directory once a day bounds how long any drift that slipped through can
/// persist.
const REMOTE_REVALIDATE_INTERVAL: Duration = Duration::from_hours(24);

/// Metadata identity captured for one local file.
///
/// Besides size and modification time, the fingerprint includes the file's
/// change time and inode on Unix. User-space writes cannot avoid bumping the
/// kernel-stamped ctime, so timestamp-restoring tools or a clock-skewed
/// network filesystem cannot smuggle changed content past the fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileFingerprint {
	/// File size in bytes.
	pub size: u64,
	/// Whole seconds of the modification time since the Unix epoch.
	pub mtime_secs: u64,
	/// Subsecond nanoseconds of the modification time.
	pub mtime_nanos: u32,
	/// Whole seconds of the change time since the Unix epoch; zero on non-Unix.
	pub ctime_secs: i64,
	/// Subsecond nanoseconds of the change time; zero on non-Unix.
	pub ctime_nanos: u32,
	/// Inode number of the file; zero on non-Unix.
	pub inode: u64,
}

/// Returns the change time and inode used to harden fingerprints on Unix.
#[cfg(unix)]
fn change_identity(metadata: &fs::Metadata) -> Option<(i64, u32, u64)> {
	use std::os::unix::fs::MetadataExt as _;
	Some((
		metadata.ctime(),
		u32::try_from(metadata.ctime_nsec()).ok()?,
		metadata.ino(),
	))
}

/// Returns a neutral change identity on platforms without ctime and inodes.
#[cfg(not(unix))]
fn change_identity(_metadata: &fs::Metadata) -> Option<(i64, u32, u64)> {
	Some((0, 0, 0))
}

impl FileFingerprint {
	/// Captures a cache-stable fingerprint from local file metadata.
	///
	/// Returns `None` when the metadata cannot form a trustworthy fingerprint:
	/// the modification time is unavailable, predates the Unix epoch, or is
	/// recent enough that a same-fingerprint rewrite could go unnoticed.
	#[must_use]
	pub(super) fn capture(metadata: &fs::Metadata) -> Option<Self> {
		let mtime = metadata.modified().ok()?;
		let since_epoch = mtime.duration_since(UNIX_EPOCH).ok()?;
		let age = SystemTime::now().duration_since(mtime).ok()?;
		if age < RACY_MTIME_WINDOW {
			return None;
		}
		let (ctime_secs, ctime_nanos, inode) = change_identity(metadata)?;
		Some(Self {
			size: metadata.len(),
			mtime_secs: since_epoch.as_secs(),
			mtime_nanos: since_epoch.subsec_nanos(),
			ctime_secs,
			ctime_nanos,
			inode,
		})
	}
}

/// One cached local file hash and the fingerprint it was computed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CachedFileState {
	/// Fingerprint of the local file when its hash was computed.
	#[serde(flatten)]
	pub fingerprint: FileFingerprint,
	/// SHA-256 hash of the file content.
	pub hash: String,
}

/// Metadata identity captured for one remote file.
///
/// Like local fingerprints, this covers size, modification time, and change
/// time. User-space writes cannot avoid bumping the kernel-stamped ctime, so a
/// timestamp-restoring tool cannot smuggle changed content past the
/// fingerprint. It is still weaker than the local one — there is no inode
/// component, and the timestamps are whatever the remote filesystem reports —
/// which is why [`is_settled`](Self::is_settled) refuses to cache freshly
/// touched files and why periodic revalidation re-hashes everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RemoteFingerprint {
	/// File size in bytes as printed by the remote inventory.
	pub size: u64,
	/// Modification time token exactly as printed by the remote inventory.
	///
	/// Kept as the raw `seconds[.fraction]` text so the comparison never
	/// depends on parsing or re-formatting a floating point timestamp.
	pub mtime: String,
	/// Change time token exactly as printed by the remote inventory.
	pub ctime: String,
}

/// Returns whether a remote inventory timestamp token is a usable cache key.
///
/// The remote inventory prints timestamps as `seconds[.fraction]`. GNU `find`
/// prints a leading `-` for times before the Unix epoch, which the comparison
/// against the remote clock cannot reason about, so such tokens are rejected
/// and the file is simply never cached.
fn is_timestamp_token(token: &str) -> bool {
	let is_digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
	match token.split_once('.') {
		// A fractional part must be present in full when the separator is.
		Some((seconds, fraction)) => is_digits(seconds) && is_digits(fraction),
		None => is_digits(token),
	}
}

impl RemoteFingerprint {
	/// Builds a fingerprint from one remote inventory metadata record.
	///
	/// Returns `None` when any token cannot serve as a cache key, which callers
	/// treat as "always re-hash this file" rather than as an error. A pre-epoch
	/// timestamp or an out-of-range size is unusual but perfectly valid on the
	/// remote side, so it must never fail a sync.
	#[must_use]
	pub(super) fn parse(size: &str, mtime: &str, ctime: &str) -> Option<Self> {
		if !is_timestamp_token(mtime) || !is_timestamp_token(ctime) {
			return None;
		}
		Some(Self {
			size: size.parse::<u64>().ok()?,
			mtime: mtime.to_owned(),
			ctime: ctime.to_owned(),
		})
	}

	/// Returns a validated timestamp rounded up to the next whole second.
	///
	/// A value that does not fit in an [`i64`] is reported as the maximum, which
	/// makes [`is_settled`](Self::is_settled) reject it.
	fn timestamp_ceiling_secs(token: &str) -> i64 {
		let (seconds, fraction) = token.split_once('.').unwrap_or((token, ""));
		let seconds = seconds.parse::<i64>().unwrap_or(i64::MAX);
		// The inventory clock has no fraction. Rounding a file timestamp down
		// would make the exclusion window end up to one second too early.
		if fraction.bytes().any(|byte| byte != b'0') {
			seconds.saturating_add(1)
		} else {
			seconds
		}
	}

	/// Returns whether both timestamps are old enough to key a cached hash.
	///
	/// Remote filesystems may stamp times coarsely, so a file rewritten moments
	/// after the inventory ran could land on the same tick and keep an identical
	/// fingerprint while its content changed.
	///
	/// The comparison uses the clock of the host that ran the inventory, which
	/// on a networked filesystem is not the host that stamped the timestamps.
	/// The window is therefore a heuristic, not a guarantee: a file server
	/// running ahead of the login node makes biwa cache less than it could,
	/// while one running behind can let the window pass early. The change time
	/// component of the fingerprint, not this window, is what actually makes a
	/// rewrite visible.
	pub(super) fn is_settled(&self, remote_clock_secs: i64) -> bool {
		[self.mtime.as_str(), self.ctime.as_str()]
			.into_iter()
			.all(|token| {
				let Some(age) = remote_clock_secs.checked_sub(Self::timestamp_ceiling_secs(token))
				else {
					return false;
				};
				// A negative age means the timestamp is ahead of the remote clock.
				u64::try_from(age).is_ok_and(|age| age >= RACY_MTIME_WINDOW_SECS)
			})
	}
}

/// One cached remote file hash and the fingerprint it was observed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CachedRemoteState {
	/// Fingerprint of the remote file when its hash was observed.
	#[serde(flatten)]
	pub fingerprint: RemoteFingerprint,
	/// SHA-256 hash of the remote file content.
	pub hash: String,
}

/// Cached synchronization state for one cache key.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SyncCacheData {
	/// Cached local file states keyed by path relative to the sync root.
	pub local_files: BTreeMap<String, CachedFileState>,
	/// Cached remote file states keyed by path relative to the remote directory.
	pub remote_files: BTreeMap<String, CachedRemoteState>,
	/// Unix seconds of the last full remote hash pass, if one was ever recorded.
	pub remote_hashed_at: Option<u64>,
}

/// Identity of one cache scope: SSH target plus both synchronization roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CacheKey {
	/// Effective SSH hostname.
	pub host: String,
	/// Effective SSH port.
	pub port: u16,
	/// Effective SSH username.
	pub user: String,
	/// Absolute local sync root.
	pub sync_root: PathBuf,
	/// Resolved remote project directory.
	pub remote_dir: String,
}

impl CacheKey {
	/// Builds a cache key for a resolved SSH target and transfer roots.
	#[must_use]
	pub(super) fn new(target: &ResolvedSshTarget, sync_root: &Path, remote_dir: &str) -> Self {
		Self {
			host: target.hostname.clone(),
			port: target.port,
			user: target.user.clone(),
			sync_root: sync_root.to_path_buf(),
			remote_dir: remote_dir.to_owned(),
		}
	}

	/// Returns the cache file name for this key.
	///
	/// The name is a digest of every identity component so distinct targets,
	/// local roots, and remote directories never share a cache file.
	fn file_name(&self) -> String {
		let mut hasher = Sha256::new();
		for component in [
			self.host.as_str(),
			&self.port.to_string(),
			self.user.as_str(),
			&self.sync_root.to_string_lossy(),
			self.remote_dir.as_str(),
		] {
			hasher.update(component.as_bytes());
			hasher.update([0]);
		}
		format!("{}.json", hex::encode(hasher.finalize()))
	}
}

/// Persisted sync cache contents for one cache key.
#[derive(Debug, Serialize, Deserialize)]
struct SyncCacheFile {
	/// On-disk format version.
	version: u32,
	/// Effective SSH hostname the cache was written for.
	host: String,
	/// Effective SSH port the cache was written for.
	port: u16,
	/// Effective SSH username the cache was written for.
	user: String,
	/// Absolute local sync root the cache was written for.
	sync_root: PathBuf,
	/// Resolved remote project directory the cache was written for.
	remote_dir: String,
	/// Cached local file states keyed by path relative to the sync root.
	files: BTreeMap<String, CachedFileState>,
	/// Cached remote file states keyed by path relative to the remote directory.
	remote_files: BTreeMap<String, CachedRemoteState>,
	/// Unix seconds of the last full remote hash pass.
	remote_hashed_at: Option<u64>,
}

/// Returns the current Unix time in whole seconds.
///
/// Returns `None` when the system clock predates the Unix epoch, which callers
/// treat as a reason to revalidate rather than to trust cached remote hashes.
#[must_use]
pub(super) fn unix_now() -> Option<u64> {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.ok()
		.map(|since_epoch| since_epoch.as_secs())
}

/// Returns whether the remote hash cache is due for a full revalidation pass.
///
/// Revalidation is due when no full pass was ever recorded, when either
/// timestamp is unusable, when the clock moved backwards, or when the last pass
/// is older than [`REMOTE_REVALIDATE_INTERVAL`].
#[must_use]
pub(super) const fn remote_revalidation_due(last_full_hash: Option<u64>, now: Option<u64>) -> bool {
	let (Some(last_full_hash), Some(now)) = (last_full_hash, now) else {
		return true;
	};
	let Some(age) = now.checked_sub(last_full_hash) else {
		return true;
	};
	age >= REMOTE_REVALIDATE_INTERVAL.as_secs()
}

/// Resolves the sync cache directory from configuration.
///
/// Priority: `sync.sftp.cache.path` > `<state dir>/sync_cache`.
#[must_use]
pub(super) fn resolve_cache_dir(config: &Config) -> PathBuf {
	config
		.sync
		.sftp
		.cache
		.path
		.clone()
		.unwrap_or_else(|| config.resolved_state_dir().join(CACHE_DIR_NAME))
}

/// Returns the cache file path for a key inside a cache directory.
#[must_use]
fn cache_file_path(cache_dir: &Path, key: &CacheKey) -> PathBuf {
	cache_dir.join(key.file_name())
}

/// Loads cached file states for a key, tolerating any unusable cache.
///
/// Returns `None` when the cache file is missing, unreadable, unparsable, has
/// an unexpected version, or was written for a different identity. Every
/// fallback reason is logged at debug level; a bad cache never fails a sync.
#[must_use]
pub(super) fn load_cache(cache_dir: &Path, key: &CacheKey) -> Option<SyncCacheData> {
	let path = cache_file_path(cache_dir, key);
	let contents = match fs::read_to_string(&path) {
		Ok(contents) => contents,
		Err(error) if error.kind() == ErrorKind::NotFound => {
			debug!(path = %path.display(), "No sync cache file; starting from a full scan");
			return None;
		}
		Err(error) => {
			debug!(path = %path.display(), %error, "Failed to read sync cache; falling back to a full scan");
			return None;
		}
	};
	let cache = match serde_json::from_str::<SyncCacheFile>(&contents) {
		Ok(cache) => cache,
		Err(error) => {
			debug!(path = %path.display(), %error, "Failed to parse sync cache; falling back to a full scan");
			return None;
		}
	};
	if cache.version != CACHE_VERSION {
		debug!(
			path = %path.display(),
			version = cache.version,
			expected = CACHE_VERSION,
			"Sync cache version mismatch; falling back to a full scan"
		);
		return None;
	}
	if cache.host != key.host
		|| cache.port != key.port
		|| cache.user != key.user
		|| cache.sync_root != key.sync_root
		|| cache.remote_dir != key.remote_dir
	{
		debug!(path = %path.display(), "Sync cache identity mismatch; falling back to a full scan");
		return None;
	}
	debug!(
		path = %path.display(),
		local_entries = cache.files.len(),
		remote_entries = cache.remote_files.len(),
		remote_hashed_at = cache.remote_hashed_at,
		"Loaded sync cache"
	);
	Some(SyncCacheData {
		local_files: cache.files,
		remote_files: cache.remote_files,
		remote_hashed_at: cache.remote_hashed_at,
	})
}

/// Minimum age before an orphaned temporary cache file is swept.
///
/// A live save holds its temporary file only for milliseconds, so anything
/// this old was abandoned by a crashed process.
const STALE_TMP_MAX_AGE: Duration = Duration::from_secs(3600);

/// Returns whether a file name has the exact shape of biwa's temporary cache files.
///
/// Temporary cache files are named `<64 hex digest>.json.<pid>.tmp`. Sweeping
/// only this shape guarantees files written by other tools are never removed,
/// even when `sync.sftp.cache.path` points at a shared directory.
fn is_cache_tmp_file_name(name: &str) -> bool {
	let Some(rest) = name.strip_suffix(".tmp") else {
		return false;
	};
	let Some((digest, pid)) = rest.split_once(".json.") else {
		return false;
	};
	digest.len() == 64
		&& digest.chars().all(|char| char.is_ascii_hexdigit())
		&& !pid.is_empty()
		&& pid.chars().all(|char| char.is_ascii_digit())
}

/// Removes orphaned temporary cache files left behind by crashed saves, best-effort.
///
/// Only files matching biwa's temporary cache file naming and older than
/// [`STALE_TMP_MAX_AGE`] are removed, so a concurrent save's in-flight
/// temporary file and foreign files in a shared directory are never deleted.
fn sweep_stale_tmp_files(cache_dir: &Path) {
	let Ok(entries) = fs::read_dir(cache_dir) else {
		return;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if !entry
			.file_name()
			.to_str()
			.is_some_and(is_cache_tmp_file_name)
		{
			continue;
		}
		let is_stale = entry
			.metadata()
			.and_then(|metadata| metadata.modified())
			.is_ok_and(|mtime| {
				SystemTime::now()
					.duration_since(mtime)
					.is_ok_and(|age| age >= STALE_TMP_MAX_AGE)
			});
		if is_stale && fs::remove_file(&path).is_ok() {
			debug!(path = %path.display(), "Removed stale temporary sync cache file");
		}
	}
}

/// Saves cached file states for a key atomically.
///
/// The cache is written to a temporary file first and renamed into place so a
/// concurrent reader never observes a partially written cache.
pub(super) fn save_cache(cache_dir: &Path, key: &CacheKey, data: SyncCacheData) -> Result<()> {
	let path = cache_file_path(cache_dir, key);
	fs::create_dir_all(cache_dir).wrap_err_with(|| {
		format!(
			"Failed to create sync cache directory: {}",
			cache_dir.display()
		)
	})?;
	sweep_stale_tmp_files(cache_dir);
	let cache = SyncCacheFile {
		version: CACHE_VERSION,
		host: key.host.clone(),
		port: key.port,
		user: key.user.clone(),
		sync_root: key.sync_root.clone(),
		remote_dir: key.remote_dir.clone(),
		files: data.local_files,
		remote_files: data.remote_files,
		remote_hashed_at: data.remote_hashed_at,
	};
	let contents = serde_json::to_string(&cache).wrap_err("Failed to serialize sync cache")?;
	let tmp_path = cache_dir.join(format!("{}.{}.tmp", key.file_name(), process::id()));
	fs::write(&tmp_path, &contents)
		.wrap_err_with(|| format!("Failed to write sync cache: {}", tmp_path.display()))?;
	fs::rename(&tmp_path, &path).wrap_err_with(|| {
		format!(
			"Failed to rename sync cache file: {} -> {}",
			tmp_path.display(),
			path.display()
		)
	})?;
	debug!(
		path = %path.display(),
		local_entries = cache.files.len(),
		remote_entries = cache.remote_files.len(),
		"Saved sync cache"
	);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use core::time::Duration;
	use pretty_assertions::{assert_eq, assert_ne};
	use std::fs::File;
	use tempfile::tempdir;

	/// Builds a representative cache key for tests.
	fn test_key() -> CacheKey {
		CacheKey {
			host: "cse.unsw.edu.au".to_owned(),
			port: 22,
			user: "z5555555".to_owned(),
			sync_root: PathBuf::from("/home/user/project"),
			remote_dir: "~/.cache/biwa/projects/project-abc".to_owned(),
		}
	}

	/// Builds one cached file state with an arbitrary fingerprint.
	fn cached_file(hash: &str) -> CachedFileState {
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

	/// Builds one cached remote file state with an arbitrary fingerprint.
	fn cached_remote_file(hash: &str) -> CachedRemoteState {
		CachedRemoteState {
			fingerprint: RemoteFingerprint {
				size: 5,
				mtime: "1700000000.1234567890".to_owned(),
				ctime: "1700000000.1234567890".to_owned(),
			},
			hash: hash.to_owned(),
		}
	}

	/// Builds cache data holding one local and one remote entry.
	fn test_data() -> SyncCacheData {
		SyncCacheData {
			local_files: BTreeMap::from([("src/main.rs".to_owned(), cached_file("abc123"))]),
			remote_files: BTreeMap::from([(
				"src/main.rs".to_owned(),
				cached_remote_file("def456"),
			)]),
			remote_hashed_at: Some(1_700_000_000),
		}
	}

	#[test]
	fn save_and_load_round_trips() {
		let dir = tempdir().unwrap();
		let key = test_key();
		let data = test_data();

		save_cache(dir.path(), &key, data.clone()).unwrap();

		assert_eq!(load_cache(dir.path(), &key), Some(data));
	}

	#[test]
	fn save_leaves_no_temporary_files() {
		let dir = tempdir().unwrap();
		let key = test_key();

		save_cache(dir.path(), &key, SyncCacheData::default()).unwrap();

		let names = fs::read_dir(dir.path())
			.unwrap()
			.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
			.collect::<Vec<_>>();
		assert_eq!(names, vec![key.file_name()]);
	}

	#[test]
	fn cache_files_are_scoped_by_every_identity_component() {
		let base = test_key();
		let variants = [
			CacheKey {
				host: "other.example.org".to_owned(),
				..base.clone()
			},
			CacheKey {
				port: 2222,
				..base.clone()
			},
			CacheKey {
				user: "z1111111".to_owned(),
				..base.clone()
			},
			CacheKey {
				sync_root: PathBuf::from("/home/user/other"),
				..base.clone()
			},
			CacheKey {
				remote_dir: "~/.cache/biwa/projects/project-def".to_owned(),
				..base.clone()
			},
		];

		for variant in variants {
			assert_ne!(variant.file_name(), base.file_name(), "{variant:?}");
		}
	}

	#[test]
	fn load_returns_none_for_missing_cache() {
		let dir = tempdir().unwrap();

		assert_eq!(load_cache(dir.path(), &test_key()), None);
	}

	#[test]
	fn load_rejects_corrupt_cache() {
		let dir = tempdir().unwrap();
		let key = test_key();
		fs::write(cache_file_path(dir.path(), &key), "{not json").unwrap();

		assert_eq!(load_cache(dir.path(), &key), None);
	}

	#[test]
	fn load_rejects_version_mismatch() {
		let dir = tempdir().unwrap();
		let key = test_key();
		save_cache(dir.path(), &key, test_data()).unwrap();
		let path = cache_file_path(dir.path(), &key);
		let mut value =
			serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap();
		value
			.as_object_mut()
			.unwrap()
			.insert("version".to_owned(), serde_json::json!(999));
		fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

		assert_eq!(load_cache(dir.path(), &key), None);
	}

	#[test]
	fn load_rejects_identity_mismatch() {
		let dir = tempdir().unwrap();
		let original = test_key();
		let other = CacheKey {
			remote_dir: "~/.cache/biwa/projects/project-def".to_owned(),
			..original.clone()
		};
		save_cache(dir.path(), &original, test_data()).unwrap();
		// Simulate a cache written for a different identity at this key's path.
		fs::copy(
			cache_file_path(dir.path(), &original),
			cache_file_path(dir.path(), &other),
		)
		.unwrap();

		assert_eq!(load_cache(dir.path(), &other), None);
	}

	#[test]
	fn fingerprint_capture_skips_recently_modified_files() {
		let dir = tempdir().unwrap();
		let path = dir.path().join("fresh.txt");
		fs::write(&path, "content").unwrap();

		assert_eq!(
			FileFingerprint::capture(&fs::symlink_metadata(&path).unwrap()),
			None
		);
	}

	#[test]
	fn fingerprint_capture_returns_stable_fingerprint() {
		let dir = tempdir().unwrap();
		let path = dir.path().join("old.txt");
		fs::write(&path, "content").unwrap();
		let mtime = SystemTime::now()
			.checked_sub(Duration::from_secs(60))
			.unwrap();
		File::options()
			.append(true)
			.open(&path)
			.unwrap()
			.set_modified(mtime)
			.unwrap();

		let metadata = fs::symlink_metadata(&path).unwrap();
		let fingerprint = FileFingerprint::capture(&metadata).unwrap();

		assert_eq!(fingerprint.size, u64::try_from("content".len()).unwrap());
		let expected = mtime.duration_since(UNIX_EPOCH).unwrap();
		assert_eq!(fingerprint.mtime_secs, expected.as_secs());
		assert_eq!(fingerprint.mtime_nanos, expected.subsec_nanos());
		#[cfg(unix)]
		{
			use std::os::unix::fs::MetadataExt as _;
			assert_eq!(fingerprint.inode, metadata.ino());
			assert_eq!(fingerprint.ctime_secs, metadata.ctime());
		}
	}

	#[test]
	fn save_sweeps_only_stale_biwa_temporary_files() {
		let dir = tempdir().unwrap();
		let digest = "0".repeat(64);
		let backdate = |path: &Path| {
			let stale_mtime = SystemTime::now()
				.checked_sub(Duration::from_secs(7200))
				.unwrap();
			File::options()
				.append(true)
				.open(path)
				.unwrap()
				.set_modified(stale_mtime)
				.unwrap();
		};
		let stale_tmp = dir.path().join(format!("{digest}.json.123.tmp"));
		fs::write(&stale_tmp, "{}").unwrap();
		backdate(&stale_tmp);
		let fresh_tmp = dir.path().join(format!("{digest}.json.456.tmp"));
		fs::write(&fresh_tmp, "{}").unwrap();
		let foreign_tmp = dir.path().join("other-tool.tmp");
		fs::write(&foreign_tmp, "{}").unwrap();
		backdate(&foreign_tmp);

		let key = test_key();
		save_cache(dir.path(), &key, SyncCacheData::default()).unwrap();

		// Only biwa-shaped, hour-old temporary files are removed: a concurrent
		// save's fresh file and another tool's file both survive.
		assert!(!stale_tmp.exists());
		assert!(fresh_tmp.exists());
		assert!(foreign_tmp.exists());
		assert!(cache_file_path(dir.path(), &key).exists());
	}

	#[test]
	fn load_rejects_older_cache_versions() {
		let dir = tempdir().unwrap();
		let key = test_key();
		for version in [1, 2] {
			// Older files predate parts of the remote inventory cache and must be
			// ignored rather than parsed with missing remote state.
			fs::write(
				cache_file_path(dir.path(), &key),
				serde_json::json!({
					"version": version,
					"host": key.host,
					"port": key.port,
					"user": key.user,
					"sync_root": key.sync_root,
					"remote_dir": key.remote_dir,
					"files": {},
					"remote_files": {},
					"remote_hashed_at": 1_700_000_000_u64,
				})
				.to_string(),
			)
			.unwrap();

			assert_eq!(load_cache(dir.path(), &key), None, "version {version}");
		}
	}

	#[test]
	fn remote_fingerprint_parses_inventory_metadata() {
		let fingerprint =
			RemoteFingerprint::parse("1234", "1700000000.1234567890", "1700000001.5").unwrap();

		assert_eq!(fingerprint.size, 1234);
		assert_eq!(fingerprint.mtime, "1700000000.1234567890");
		assert_eq!(fingerprint.ctime, "1700000001.5");
		assert_eq!(
			RemoteFingerprint::timestamp_ceiling_secs(&fingerprint.mtime),
			1_700_000_001
		);
		// A timestamp without a fractional part stays usable.
		let whole = RemoteFingerprint::parse("0", "1700000000", "1700000000").unwrap();
		assert_eq!(
			RemoteFingerprint::timestamp_ceiling_secs(&whole.mtime),
			1_700_000_000
		);
	}

	#[test]
	fn remote_fingerprint_rejects_unusable_metadata() {
		for (size, mtime, ctime) in [
			("-1", "1700000000.0", "1700000000.0"),
			("1e3", "1700000000.0", "1700000000.0"),
			("10", "now", "1700000000.0"),
			("10", "1700000000.", "1700000000.0"),
			("10", ".5", "1700000000.0"),
			("10", "1700000000.5x", "1700000000.0"),
			("10", "1700000000.0", "not-a-time"),
			// GNU find prints pre-epoch timestamps with a leading sign. Such a
			// file is perfectly valid remotely, so it must degrade to "never
			// cached" instead of failing the sync.
			("10", "-315576000.0000000000", "1700000000.0"),
			("10", "1700000000.0", "-315576000.0000000000"),
		] {
			assert_eq!(
				RemoteFingerprint::parse(size, mtime, ctime),
				None,
				"accepted {size} {mtime} {ctime}"
			);
		}
	}

	#[test]
	fn remote_fingerprint_is_settled_only_outside_the_racy_window() {
		let fingerprint = RemoteFingerprint::parse("10", "1700000000.5", "1700000000.5").unwrap();

		// At this whole-second clock value the timestamp is only 1.5 seconds
		// old, so dropping its fractional part must not open the window early.
		assert!(!fingerprint.is_settled(1_700_000_002));
		assert!(fingerprint.is_settled(1_700_000_003));
		assert!(!fingerprint.is_settled(1_700_000_001));
		// A timestamp ahead of the remote clock is never trusted.
		assert!(!fingerprint.is_settled(1_699_999_000));
		// A settled modification time does not excuse a fresh change time: a
		// rewrite that restored the mtime still bumps the ctime.
		let restored = RemoteFingerprint::parse("10", "1700000000.5", "1700000002.5").unwrap();
		assert!(!restored.is_settled(1_700_000_003));
		assert!(restored.is_settled(1_700_000_005));
		let whole = RemoteFingerprint::parse("10", "1700000000", "1700000000.000").unwrap();
		assert!(whole.is_settled(1_700_000_002));
		let tiny_fraction =
			RemoteFingerprint::parse("10", "1700000000.000000001", "1700000000").unwrap();
		assert!(!tiny_fraction.is_settled(1_700_000_002));
		assert!(tiny_fraction.is_settled(1_700_000_003));
	}

	#[test]
	fn remote_fingerprint_rejects_ages_that_overflow() {
		for timestamp in ["9223372036854775807.5", "9223372036854775808"] {
			let fingerprint = RemoteFingerprint::parse("10", timestamp, timestamp).unwrap();
			assert!(!fingerprint.is_settled(i64::MAX));
			assert!(!fingerprint.is_settled(i64::MIN));
		}
	}

	#[test]
	fn remote_revalidation_is_due_without_a_recorded_pass_or_after_the_interval() {
		let interval = REMOTE_REVALIDATE_INTERVAL.as_secs();
		let now = 1_700_000_000;

		assert!(remote_revalidation_due(None, Some(now)));
		assert!(remote_revalidation_due(Some(now), None));
		assert!(remote_revalidation_due(
			Some(now.saturating_sub(interval)),
			Some(now)
		));
		// A clock that moved backwards is treated as unusable.
		assert!(remote_revalidation_due(Some(now), Some(now - 1)));
		assert!(!remote_revalidation_due(
			Some(now.saturating_sub(interval - 1)),
			Some(now)
		));
	}
}
