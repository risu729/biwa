use crate::Result;
use crate::config::types::Config;
use color_eyre::eyre::{Context as _, ContextCompat as _, bail};
use core::cmp::Ordering;
use core::mem::take;
use gethostname::gethostname;
use sha2::{Digest as _, Sha256};
use std::path::Path;

/// Conservative upper bound for a batched remote `mkdir -p` command.
pub(super) const MAX_REMOTE_MKDIR_COMMAND_LEN: usize = 4096;

/// Shell-quotes a remote path while preserving home directory expansion.
///
/// If the path starts with `~/`, the `~` is replaced with `$HOME` and placed
/// outside the quotes so the shell can expand it. Otherwise, the entire path
/// is quoted with `shell_words::quote`.
pub(super) fn shell_quote_path(path: &str) -> String {
	if let Some(rest) = remote_home_relative_path(path) {
		if rest.is_empty() {
			return "\"$HOME\"".to_owned();
		}
		return format!("\"$HOME\"/{}", shell_words::quote(rest));
	}
	shell_words::quote(path).into_owned()
}

/// Lexically normalizes a remote target without permitting parent traversal.
pub(super) fn normalize_remote_dir(remote_dir: &str) -> Result<String> {
	if remote_dir.is_empty() {
		bail!("Remote directory must not be empty");
	}

	let (prefix, remainder) = if remote_dir == "$HOME" {
		("$HOME", "")
	} else if let Some(remainder) = remote_dir.strip_prefix("$HOME/") {
		("$HOME", remainder)
	} else if remote_dir == "~" {
		("~", "")
	} else if let Some(remainder) = remote_dir.strip_prefix("~/") {
		("~", remainder)
	} else if let Some(remainder) = remote_dir.strip_prefix('/') {
		("/", remainder)
	} else {
		("", remote_dir)
	};
	let mut components = Vec::new();
	for component in remainder.split('/') {
		match component {
			"" | "." => {}
			".." => {
				bail!("Remote directory must not contain parent traversal (`..`): {remote_dir}")
			}
			other => components.push(other),
		}
	}

	let suffix = components.join("/");
	if prefix == "/" {
		return Ok(if suffix.is_empty() {
			"/".to_owned()
		} else {
			format!("/{suffix}")
		});
	}
	if prefix.is_empty() {
		return Ok(if suffix.is_empty() {
			".".to_owned()
		} else {
			suffix
		});
	}
	Ok(if suffix.is_empty() {
		prefix.to_owned()
	} else {
		format!("{prefix}/{suffix}")
	})
}

/// Returns a normalized path relative to the remote home directory.
fn remote_home_relative_path(path: &str) -> Option<&str> {
	if path == "~" || path == "$HOME" {
		return Some("");
	}
	path.strip_prefix("~/")
		.or_else(|| path.strip_prefix("$HOME/"))
		.map(|rest| rest.trim_start_matches('/'))
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

/// Returns the 8-character hex hash of the local machine hostname.
///
/// Used to identify which remote directories belong to this client.
#[must_use]
pub(super) fn compute_client_host_hash() -> String {
	let hash = hex::encode(Sha256::digest(gethostname().to_string_lossy().as_bytes()));
	#[expect(
		clippy::string_slice,
		reason = "Hex encoded strings are strictly ASCII, slicing is safe"
	)]
	hash[..8].to_owned()
}

/// Computes a unique project name based on the hostname and project root's canonical path.
///
/// The format is `{project_name}-{host_hash}-{path_hash}` where each hash is
/// an 8-character hex prefix. The host hash is separate so the clean feature
/// can identify directories belonging to this client.
fn compute_unique_project_name(project_root: &Path) -> Result<String> {
	let project_name = project_root
		.file_name()
		.wrap_err("Invalid project root directory")?
		.to_string_lossy()
		.into_owned();

	let host_hash = compute_client_host_hash();
	let path_hash = hex::encode(Sha256::digest(
		project_root
			.canonicalize()
			.wrap_err("Failed to canonicalize project root")?
			.to_string_lossy()
			.as_bytes(),
	));
	#[expect(
		clippy::string_slice,
		reason = "Hex encoded strings are strictly ASCII, slicing is safe"
	)]
	Ok(format!(
		"{}-{}-{}",
		project_name,
		host_hash,
		&path_hash[..8]
	))
}

/// Computes the remote directory path for a given project.
///
/// This is the directory where synced files are stored on the remote server.
pub(super) fn compute_project_remote_dir(config: &Config, project_root: &Path) -> Result<String> {
	let unique_project_name = compute_unique_project_name(project_root)?;
	let remote_dir = compute_remote_path(
		&config.sync.remote_root,
		&unique_project_name,
		Path::new(""),
	);
	normalize_remote_dir(&remote_dir)
}

/// Returns true if `remote_dir` is directly under `remote_root` and its final path component
/// follows biwa's default `{project_name}-{host_hash}-{path_hash}` layout.
///
/// Used to avoid deleting arbitrary paths from local state during `biwa clean --auto` / `--all`.
#[must_use]
pub(super) fn is_default_biwa_remote_dir(
	remote_dir: &str,
	remote_root: &Path,
	host_hash: &str,
) -> bool {
	if !is_hex_hash(host_hash) {
		return false;
	}

	let Some(directory_name) = direct_remote_child_name(remote_dir, remote_root) else {
		return false;
	};

	let Some((project_and_host, path_hash)) = directory_name.rsplit_once('-') else {
		return false;
	};
	if !is_hex_hash(path_hash) {
		return false;
	}
	let Some((project_name, actual_host_hash)) = project_and_host.rsplit_once('-') else {
		return false;
	};

	!project_name.is_empty() && actual_host_hash.eq_ignore_ascii_case(host_hash)
}

/// Returns true if `remote_dir` is directly under `remote_root` and looks like a biwa project dir.
///
/// Accepts both the current `{project_name}-{host_hash}-{path_hash}` layout and the legacy
/// `{project_name}-{combined_hash}` layout so `biwa clean --purge` can remove old default dirs
/// without deleting arbitrary siblings under `remote_root`.
#[must_use]
pub(super) fn is_biwa_remote_dir(remote_dir: &str, remote_root: &Path) -> bool {
	let Some(directory_name) = direct_remote_child_name(remote_dir, remote_root) else {
		return false;
	};

	let Some((project_name, path_hash)) = directory_name.rsplit_once('-') else {
		return false;
	};

	!project_name.is_empty() && is_hex_hash(path_hash)
}

/// Returns the direct child component of a remote path under `remote_root`.
fn direct_remote_child_name(remote_dir: &str, remote_root: &Path) -> Option<String> {
	let remote_dir = normalize_remote_dir(remote_dir).ok()?;
	let root = normalize_remote_dir(&remote_root.to_string_lossy()).ok()?;
	let directory_name = if root == "." {
		remote_dir.as_str()
	} else {
		let prefix = format!("{root}/");
		remote_dir.strip_prefix(&prefix)?
	};
	if directory_name.is_empty()
		|| directory_name == "."
		|| directory_name == ".."
		|| directory_name.contains('/')
	{
		return None;
	}

	Some(directory_name.to_owned())
}

/// Returns whether a string is exactly 8 ASCII hex characters.
fn is_hex_hash(value: &str) -> bool {
	value.len() == 8 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// SFTP naturally resolves paths not starting with `/` relative to the user's home directory.
/// It does NOT expand `~/` or `$HOME/` like a shell would. Therefore, we strip them so SFTP
/// looks in the home directory instead of looking for literal `~` or `$HOME` folders.
pub(super) fn resolve_sftp_path(remote_path: &str) -> &str {
	remote_home_relative_path(remote_path)
		.map_or(remote_path, |path| if path.is_empty() { "." } else { path })
}

/// Compares remote paths by slash-separated components.
fn compare_remote_paths(left: &str, right: &str) -> Ordering {
	let mut left_components = left.split('/');
	let mut right_components = right.split('/');

	loop {
		match (left_components.next(), right_components.next()) {
			(Some(left_component), Some(right_component)) => {
				let ordering = left_component.cmp(right_component);
				if ordering != Ordering::Equal {
					return ordering;
				}
			}
			(None, Some(_)) => return Ordering::Less,
			(Some(_), None) => return Ordering::Greater,
			(None, None) => return Ordering::Equal,
		}
	}
}

/// Returns whether `candidate` is nested beneath `ancestor`.
fn is_nested_directory(candidate: &str, ancestor: &str) -> bool {
	candidate != ancestor && Path::new(candidate).starts_with(ancestor)
}

/// Reduces a directory set to its deepest leaf paths.
pub(super) fn collect_leaf_directories(paths: &[String]) -> Vec<String> {
	if paths.is_empty() {
		return Vec::new();
	}

	let mut sorted = paths.to_vec();
	sorted.sort_unstable_by(|left, right| compare_remote_paths(left, right));
	sorted.dedup();

	let mut leaves = Vec::new();
	for (index, current) in sorted.iter().enumerate() {
		let is_leaf = sorted
			.get(index.saturating_add(1))
			.is_none_or(|next| !is_nested_directory(next, current));
		if is_leaf {
			leaves.push(current.clone());
		}
	}

	leaves
}

/// Splits directory creation into bounded `mkdir -p` batches.
pub(super) fn build_mkdir_commands(
	umask: &str,
	remote_dir: &str,
	relative_paths: &[String],
) -> Vec<String> {
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
