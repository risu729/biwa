use super::exec::connect;
use crate::config::types::{Config, SyncEngine};
use crate::ssh::sftp::upload_file;
use crate::ui::create_spinner;
use console::style;
use eyre::{Context as _, ContextCompat as _, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use russh_sftp::client::SftpSession;
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, Read as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Statistics for a synchronization operation.
#[derive(Debug, Default, PartialEq, Eq)]
#[expect(
	clippy::module_name_repetitions,
	reason = "Plan defined it as SyncStats"
)]
#[expect(clippy::struct_field_names, reason = "Plan defined it as files_*")]
pub struct SyncStats {
	/// Number of files uploaded.
	pub files_uploaded: usize,
	/// Number of files deleted.
	pub files_deleted: usize,
	/// Number of files unchanged.
	pub files_unchanged: usize,
}

/// Options for a synchronization operation.
#[derive(Debug, Default, Clone)]
#[expect(
	clippy::module_name_repetitions,
	reason = "Plan defined it as SyncOptions"
)]
pub struct SyncOptions {
	/// Force synchronization of all files, ignoring incremental hash checks.
	pub force: bool,
	/// Exclude files matching these paths or globs.
	pub exclude: Vec<String>,
	/// Only synchronize files matching these paths or globs.
	pub include: Vec<String>,
}

/// Builds a `GlobSet` from a slice of pattern strings.
fn build_globset(patterns: &[String]) -> Result<GlobSet> {
	let mut builder = GlobSetBuilder::new();
	for pattern in patterns {
		builder.add(Glob::new(pattern)?);
	}
	builder.build().wrap_err("Failed to build glob set")
}

/// Checks if the remote root path is absolute and prints a warning.
pub fn check_remote_root(remote_root: &Path) {
	if remote_root.is_absolute() {
		warn!(
			"Absolute remote_root path detected: {}. It is recommended to use a relative path starting with '~'.",
			remote_root.display()
		);
		eprintln!(
			"{} Absolute remote_root path detected: {}. It is recommended to use a relative path starting with '~'.",
			style("Warning:").yellow().bold(),
			style(remote_root.display()).bold()
		);
	}
}

/// Collects local files from the project root, respecting ignore rules.
pub fn collect_local_files(
	root: &Path,
	extra_ignores: &[PathBuf],
	options: &SyncOptions,
) -> Result<Vec<(PathBuf, String)>> {
	let mut builder = WalkBuilder::new(root);
	builder.standard_filters(true); // .gitignore, .ignore, etc.
	builder.require_git(false); // Respect .gitignore even outside of git repositories

	let exclude_globs = build_globset(&options.exclude)?;
	let include_globs = build_globset(&options.include)?;
	let has_includes = !options.include.is_empty();

	let mut result = Vec::new();
	for entry in builder.build() {
		let entry = entry?;
		let path = entry.path();
		if path.is_file() {
			let relative = path.strip_prefix(root).wrap_err("Failed to strip prefix")?;

			let mut ignored = false;
			for ignore_path in extra_ignores {
				if relative.starts_with(ignore_path) || relative == ignore_path {
					ignored = true;
					break;
				}
			}
			if ignored {
				continue;
			}

			if exclude_globs.is_match(relative) {
				continue;
			}

			if has_includes && !include_globs.is_match(relative) {
				continue;
			}

			let file = File::open(path)?;
			let mut reader = BufReader::new(file);
			let mut hasher = Sha256::new();
			let mut buffer = [0; 8192];
			loop {
				let count = reader.read(&mut buffer)?;
				if count == 0 {
					break;
				}
				hasher.update(buffer.get(..count).wrap_err("Buffer slice out of bounds")?);
			}
			let hash = hex::encode(hasher.finalize());
			result.push((relative.to_path_buf(), hash));
		}
	}
	Ok(result)
}

/// Computes the absolute remote path for a given local file.
pub fn compute_remote_path(remote_root: &Path, project_name: &str, relative: &Path) -> String {
	let root_str = remote_root.display().to_string().replace('\\', "/");
	let mut parts = Vec::new();
	if !root_str.is_empty() {
		parts.push(root_str);
	}
	parts.push(project_name.to_owned());

	let rel_str = relative.display().to_string().replace('\\', "/");
	if !rel_str.is_empty() {
		parts.push(rel_str);
	}

	parts.join("/")
}

/// Synchronizes a project to a remote server.
#[expect(
	clippy::module_name_repetitions,
	reason = "Plan defined it as sync_project"
)]
#[expect(clippy::too_many_lines, reason = "Complex sync logic")]
#[expect(clippy::cognitive_complexity, reason = "Complex sync logic")]
#[expect(
	clippy::string_slice,
	reason = "Hex encoded strings are strictly ASCII, slicing is safe"
)]
pub async fn sync_project(
	config: &Config,
	project_root: &Path,
	options: &SyncOptions,
	quiet: bool,
) -> Result<SyncStats> {
	if config.sync.engine != SyncEngine::Sftp {
		bail!("Only SFTP sync engine is currently supported");
	}

	check_remote_root(&config.sync.remote_root);

	let project_name = project_root
		.file_name()
		.wrap_err("Invalid project root directory")?
		.to_string_lossy()
		.into_owned();

	// Create a unique hash based on the absolute path to prevent collisions between projects with the same name
	let mut hasher = Sha256::new();
	hasher.update(
		project_root
			.canonicalize()
			.wrap_err("Failed to canonicalize project root")?
			.to_string_lossy()
			.as_bytes(),
	);
	let hash_hex = hex::encode(hasher.finalize());
	let unique_project_name = format!("{}-{}", project_name, &hash_hex[..8]);

	let local_files = collect_local_files(project_root, &config.sync.ignore_files, options)?;

	let spinner = if quiet {
		None
	} else {
		Some(create_spinner("Synchronizing files...".to_owned()))
	};

	let client = connect(config, quiet).await?;

	// Compute remote directory base
	let remote_dir = compute_remote_path(
		&config.sync.remote_root,
		&unique_project_name,
		Path::new(""),
	);
	let quoted_remote_dir = shell_words::quote(&remote_dir);

	// 1. Create remote dir with 0700 and fetch current hashes
	let script = format!(
		"mkdir -p -m 0700 -- {quoted_remote_dir} && cd -- {quoted_remote_dir} 2>/dev/null && (find . -type f -exec sha256sum {{}} + || true)"
	);

	let result = client
		.execute(&script)
		.await
		.wrap_err("Failed to fetch remote state")?;
	let output = result.stdout;

	let mut remote_hashes = HashMap::new();
	for line in output.lines() {
		if let Some((hash, raw_path)) = line.split_once("  ") {
			let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
			// Validate that the remote path does not contain directory traversal components
			// to prevent malicious deletion attacks during the sync cleanup phase.
			if path.split('/').any(|comp| comp == "..") {
   				warn!("Skipping remote file with invalid path traversal components: {}", path);
   			} else {
   				remote_hashes.insert(path.to_owned(), hash.to_owned());
   			}
		}
	}

	let mut stats = SyncStats::default();

	let mut to_upload = Vec::new();
	let mut local_paths_str = HashSet::new();

	for (rel_path, local_hash) in local_files {
		let rel_path_str = rel_path.display().to_string().replace('\\', "/");
		local_paths_str.insert(rel_path_str.clone());

		if !options.force
			&& let Some(remote_hash) = remote_hashes.get(&rel_path_str)
			&& remote_hash == &local_hash
		{
			stats.files_unchanged = stats.files_unchanged.saturating_add(1);
			continue;
		}
		to_upload.push(rel_path);
	}

	if to_upload.len() > config.sync.sftp.max_files_to_sync {
		if let Some(s) = spinner {
			s.finish_and_clear();
		}
		bail!(
			"Aborting synchronization: {} files to upload exceeds the limit of {}.\nIf this is expected, increase `sync.sftp.max_files_to_sync` in your configuration.",
			to_upload.len(),
			config.sync.sftp.max_files_to_sync
		);
	}

	// Remove deleted files
	let mut to_delete = Vec::new();
	let mut remote_paths: Vec<_> = remote_hashes.keys().cloned().collect();
	remote_paths.sort_unstable(); // Sort to avoid iter_over_hash_type issue and ensure determinism
	for remote_path in remote_paths {
		if !local_paths_str.contains(&remote_path) {
			to_delete.push(remote_path);
		}
	}

	if !to_delete.is_empty() {
		let mut delete_cmds = Vec::new();
		for path in &to_delete {
			let full_path = format!("{remote_dir}/{path}");
			delete_cmds.push(format!("rm -f -- {}", shell_words::quote(&full_path)));
			stats.files_deleted = stats.files_deleted.saturating_add(1);
		}
		let delete_script = delete_cmds.join(" && ");
		client
			.execute(&delete_script)
			.await
			.wrap_err("Failed to delete remote files")?;
	}

	// Pre-create subdirectories with 0700 permissions
	let mut dirs_to_create = HashSet::new();
	for rel_path in &to_upload {
		if let Some(parent) = rel_path.parent() {
			let p_str = parent.display().to_string().replace('\\', "/");
			if !p_str.is_empty() {
				dirs_to_create.insert(format!("{remote_dir}/{p_str}"));
			}
		}
	}

	if !dirs_to_create.is_empty() {
		let mkdirs = dirs_to_create
			.into_iter()
			.map(|d| shell_words::quote(&d).into_owned())
			.collect::<Vec<_>>()
			.join(" ");
		let mkdir_cmd = format!("mkdir -p -m 0700 -- {mkdirs}");
		client
			.execute(&mkdir_cmd)
			.await
			.wrap_err("Failed to create remote directories")?;
	}

	// Upload files and change permissions to match local user permissions (removing group/other)
	if !to_upload.is_empty() {
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

		for rel_path in to_upload {
			let local_path = project_root.join(&rel_path);
			let remote_path =
				compute_remote_path(&config.sync.remote_root, &unique_project_name, &rel_path);

			// Read local permissions
			let local_mode = fs::metadata(&local_path)
				.wrap_err_with(|| format!("Failed to read metadata for {}", local_path.display()))?
				.permissions()
				.mode();
			// Preserve user permissions but clear group/other permissions
			let secure_mode = local_mode & 0o700;

			upload_file(&sftp, &local_path, &remote_path, secure_mode).await?;

			stats.files_uploaded = stats.files_uploaded.saturating_add(1);
		}
	}

	if let Some(s) = spinner {
		s.finish_and_clear();
	}
	info!("Sync completed: {:?}", stats);
	if !quiet {
		eprintln!(
			"{} Sync completed: {} uploaded, {} deleted, {} unchanged",
			style("✓").green().bold(),
			stats.files_uploaded,
			stats.files_deleted,
			stats.files_unchanged
		);
	}

	Ok(stats)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::tempdir;

	#[test]
	#[expect(clippy::unwrap_used, reason = "Tests can panic")]
	#[expect(clippy::indexing_slicing, reason = "Tests can panic")]
	fn collect_local_files_basic() {
		let dir = tempdir().unwrap();
		let file_path = dir.path().join("test.txt");
		fs::write(&file_path, "hello").unwrap();

		let files = collect_local_files(dir.path(), &[], &SyncOptions::default()).unwrap();
		assert_eq!(files.len(), 1);
		assert_eq!(files[0].0.to_string_lossy(), "test.txt");

		let expected_hash = hex::encode(Sha256::digest(b"hello"));
		assert_eq!(files[0].1, expected_hash);
	}

	#[test]
	#[expect(clippy::unwrap_used, reason = "Tests can panic")]
	fn collect_local_files_respects_gitignore() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
		fs::write(dir.path().join("ignored.txt"), "ignored").unwrap();
		fs::write(dir.path().join("kept.txt"), "kept").unwrap();

		let files = collect_local_files(dir.path(), &[], &SyncOptions::default()).unwrap();
		let names: Vec<_> = files
			.iter()
			.map(|(p, _)| p.to_string_lossy().to_string())
			.collect();
		assert!(!names.contains(&"ignored.txt".to_owned()));
		assert!(names.contains(&"kept.txt".to_owned()));
	}
	#[test]
	fn compute_remote_path_relative_check() {
		let root = Path::new("~/.cache/biwa/projects");
		let proj = "test_proj";
		let rel = Path::new("src/main.rs");
		let remote = compute_remote_path(root, proj, rel);
		assert_eq!(remote, "~/.cache/biwa/projects/test_proj/src/main.rs");
	}
}
