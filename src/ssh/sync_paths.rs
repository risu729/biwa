use crate::Result;
use crate::config::types::Config;
use color_eyre::eyre::{Context as _, ContextCompat as _};
use core::cmp::Ordering;
use core::mem::take;
use gethostname::gethostname;
use sha2::{Digest as _, Sha256};
use std::path::Path;
use tracing::warn;

/// Conservative upper bound for a batched remote `mkdir -p` command.
pub(super) const MAX_REMOTE_MKDIR_COMMAND_LEN: usize = 4096;

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
pub(super) fn compute_project_remote_dir(config: &Config, project_root: &Path) -> Result<String> {
	let unique_project_name = compute_unique_project_name(project_root)?;
	Ok(compute_remote_path(
		&config.sync.remote_root,
		&unique_project_name,
		Path::new(""),
	))
}

/// SFTP naturally resolves paths not starting with `/` relative to the user's home directory.
/// It does NOT expand `~/` or `$HOME/` like a shell would. Therefore, we strip them so SFTP
/// looks in the home directory instead of looking for literal `~` or `$HOME` folders.
pub(super) fn resolve_sftp_path(remote_path: &str) -> &str {
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

/// Normalizes a remote relative path and rejects absolute or traversal paths.
pub(super) fn parse_remote_path(raw_path: &str, entry_kind: &str) -> Option<String> {
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
