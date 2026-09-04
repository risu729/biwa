#![expect(
	clippy::tests_outside_test_module,
	reason = "https://github.com/rust-lang/rust-clippy/issues/11024"
)]
#![expect(clippy::panic_in_result_fn, reason = "color_eyre handles panics")]
#![expect(
	clippy::shadow_unrelated,
	reason = "some tests have repeated variable names"
)]

use color_eyre::eyre::eyre;
use common::{Result, ssh_port};
use core::time::Duration;
#[cfg(unix)]
use nix::sys::signal::Signal;
use pretty_assertions::{assert_eq, assert_ne};
use rstest::rstest;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::thread::sleep;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
	fs,
	path::{Path, PathBuf},
};

mod common;

fn biwa_cmd(args: &[&str], current_dir: &Path) -> duct::Expression {
	common::biwa_cmd(args).dir(current_dir)
}

fn biwa_cmd_capable(args: &[&str], current_dir: &Path) -> duct::Expression {
	common::biwa_cmd_capable(args).dir(current_dir)
}

fn biwa_cmd_tilde(args: &[&str], current_dir: &Path) -> duct::Expression {
	biwa_cmd(args, current_dir).env("BIWA_SYNC_REMOTE_ROOT", "~/.cache/biwa/projects")
}

fn biwa_cmd_capable_tilde(args: &[&str], current_dir: &Path) -> duct::Expression {
	biwa_cmd_capable(args, current_dir).env("BIWA_SYNC_REMOTE_ROOT", "~/.cache/biwa/projects")
}

#[test]
fn e2e_sync_basic() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "world")?;

	// Explicit sync
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Run with auto sync
	let output2 = biwa_cmd_tilde(
		&["run", "cat", "~/.cache/biwa/projects/hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let _stdout2 = String::from_utf8_lossy(&output2.stdout);
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let output3 = biwa_cmd_tilde(
		&["run", "cat", &format!("{remote_proj_dir}/hello.txt")],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout3 = String::from_utf8_lossy(&output3.stdout);
	assert!(output3.status.success());
	assert!(stdout3.contains("world"));
	Ok(())
}

#[test]
fn e2e_sync_replaces_remote_file_with_local_directory() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("node"))?;
	fs::write(dir.path().join("node/child.txt"), "local")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf remote > \"$1/node\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(
		String::from_utf8_lossy(
			&biwa_cmd_tilde(
				&[
					"run",
					"--skip-sync",
					"cat",
					&format!("{remote_proj_dir}/node/child.txt"),
				],
				dir.path(),
			)
			.stdout_capture()
			.stderr_capture()
			.run()?
			.stdout,
		)
		.trim(),
		"local"
	);

	Ok(())
}

#[test]
fn e2e_sync_replaces_remote_directory_with_local_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("node"), "local")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1/node\" && printf remote > \"$1/node/child.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(
		String::from_utf8_lossy(
			&biwa_cmd_tilde(
				&[
					"run",
					"--skip-sync",
					"cat",
					&format!("{remote_proj_dir}/node"),
				],
				dir.path(),
			)
			.stdout_capture()
			.stderr_capture()
			.run()?
			.stdout,
		)
		.trim(),
		"local"
	);

	Ok(())
}

#[test]
fn e2e_pull_downloads_remote_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf 'remote content' > \"$1/remote.txt\" && chmod 700 \"$1/remote.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 downloaded"), "stderr: {stderr}");
	assert_eq!(
		fs::read_to_string(dir.path().join("remote.txt"))?,
		"remote content"
	);
	#[cfg(unix)]
	assert_eq!(
		fs::metadata(dir.path().join("remote.txt"))?
			.permissions()
			.mode() & 0o777,
		0o700
	);

	Ok(())
}

#[test]
fn e2e_pull_updates_changed_and_remote_only_files() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(
		dir.path().join("generated.md"),
		"<!-- @generated by test -->\nlocal",
	)?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf '<!-- @generated by test -->\\nremote' > \"$1/generated.md\" && printf 'remote' > \"$1/source.md\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 downloaded"), "stderr: {stderr}");
	assert_eq!(
		fs::read_to_string(dir.path().join("generated.md"))?,
		"<!-- @generated by test -->\nremote"
	);
	assert_eq!(fs::read_to_string(dir.path().join("source.md"))?, "remote");

	Ok(())
}

#[test]
fn e2e_pull_overwrites_changed_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("source.md"), "local")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf 'remote' > \"$1/source.md\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 downloaded"), "stderr: {stderr}");
	assert_eq!(fs::read_to_string(dir.path().join("source.md"))?, "remote");

	Ok(())
}

#[test]
fn e2e_pull_replaces_local_file_with_remote_directory() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("node"), "local file")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1/node\" && printf remote > \"$1/node/child.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(
		fs::read_to_string(dir.path().join("node/child.txt"))?,
		"remote"
	);

	Ok(())
}

#[test]
fn e2e_pull_replaces_local_directory_with_remote_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("node"))?;
	fs::write(dir.path().join("node/child.txt"), "local child")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf remote > \"$1/node\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(fs::read_to_string(dir.path().join("node"))?, "remote");

	Ok(())
}

#[test]
fn e2e_pull_refuses_remote_file_over_non_empty_selected_directory() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("node"))?;
	fs::write(dir.path().join("node/kept.txt"), "keep")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf remote > \"$1/node\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull", "--include", "node"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("Refusing to overwrite non-empty local directory with remote file"),
		"stderr: {stderr}"
	);
	assert_eq!(
		fs::read_to_string(dir.path().join("node/kept.txt"))?,
		"keep"
	);
	assert!(dir.path().join("node").is_dir());
	assert!(
		fs::read_dir(dir.path())?.all(|entry| {
			entry.is_ok_and(|entry| {
				!entry
					.file_name()
					.to_string_lossy()
					.starts_with(".biwa-pull-")
			})
		}),
		"pull staging data was not removed"
	);

	Ok(())
}

#[test]
fn e2e_pull_deletes_local_file_missing_remotely() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("stale.txt"), "stale")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 deleted"), "stderr: {stderr}");
	assert!(!dir.path().join("stale.txt").exists());

	Ok(())
}

#[test]
fn e2e_pull_missing_remote_dir_preserves_local_files() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("local.txt"), "local")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let output = biwa_cmd_tilde(&["pull", "--remote-dir", &remote_proj_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("remote directory does not exist"),
		"stderr: {stderr}"
	);
	assert_eq!(fs::read_to_string(dir.path().join("local.txt"))?, "local");

	Ok(())
}

#[test]
fn e2e_pull_missing_remote_dir_does_not_create_explicit_root() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let sync_root = dir.path().join("new-root");
	let sync_root_arg = sync_root.to_string_lossy().into_owned();
	let remote_dir = format!("{}-missing", common::get_remote_project_dir(dir.path())?);

	let output = biwa_cmd_tilde(
		&[
			"pull",
			"--sync-root",
			&sync_root_arg,
			"--remote-dir",
			&remote_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(!sync_root.exists());

	Ok(())
}

#[test]
fn e2e_pull_creates_explicit_root_after_validating_remote() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let sync_root = dir.path().join("new-root");
	let sync_root_arg = sync_root.to_string_lossy().into_owned();
	let remote_dir = format!("{}-explicit", common::get_remote_project_dir(dir.path())?);

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf remote > \"$1/file.txt\"",
			"sh",
			&remote_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(setup_output.status.success());

	let output = biwa_cmd_tilde(
		&[
			"pull",
			"--sync-root",
			&sync_root_arg,
			"--remote-dir",
			&remote_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(fs::read_to_string(sync_root.join("file.txt"))?, "remote");
	#[cfg(unix)]
	assert_eq!(
		fs::metadata(&sync_root)?.permissions().mode() & 0o777,
		0o700
	);

	Ok(())
}

#[test]
fn e2e_pull_inventory_failure_preserves_local_files() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("protected.txt"), "local")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf remote > \"$1/protected.txt\" && chmod 000 \"$1/protected.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert_eq!(
		fs::read_to_string(dir.path().join("protected.txt"))?,
		"local"
	);

	Ok(())
}

#[test]
fn e2e_pull_creates_and_removes_empty_dirs() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("stale"))?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1/empty\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 deleted"), "stderr: {stderr}");
	assert!(dir.path().join("empty").is_dir());
	assert!(!dir.path().join("stale").exists());

	Ok(())
}

#[test]
fn e2e_pull_respects_include_scope() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("local-only.txt"), "keep")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf kept > \"$1/kept.txt\" && printf skipped > \"$1/skipped.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull", "--include", "kept.txt"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(fs::read_to_string(dir.path().join("kept.txt"))?, "kept");
	assert!(!dir.path().join("skipped.txt").exists());
	assert_eq!(
		fs::read_to_string(dir.path().join("local-only.txt"))?,
		"keep"
	);

	Ok(())
}

#[test]
fn e2e_pull_ignores_gitignored_remote_files() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf ignored > \"$1/ignored.txt\" && printf kept > \"$1/kept.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(fs::read_to_string(dir.path().join("kept.txt"))?, "kept");
	assert!(!dir.path().join("ignored.txt").exists());

	Ok(())
}

#[test]
fn e2e_pull_respects_explicit_remote_dir_sync_root_and_exclude() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let sync_root = dir.path().join("local-root");
	fs::create_dir_all(sync_root.join("excluded"))?;
	fs::write(sync_root.join("excluded/local.txt"), "local")?;
	let remote_proj_dir = common::get_remote_project_dir(&sync_root)?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1/keep\" \"$1/excluded\" && printf included > \"$1/keep/file.txt\" && printf excluded > \"$1/excluded/remote.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(
		&[
			"pull",
			"--sync-root",
			".",
			"--remote-dir",
			&remote_proj_dir,
			"--exclude",
			"excluded/**",
		],
		&sync_root,
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 downloaded"), "stderr: {stderr}");
	assert_eq!(
		fs::read_to_string(sync_root.join("keep/file.txt"))?,
		"included"
	);
	assert!(!sync_root.join("excluded/remote.txt").exists());
	assert_eq!(
		fs::read_to_string(sync_root.join("excluded/local.txt"))?,
		"local"
	);

	Ok(())
}

#[test]
fn e2e_pull_missing_relative_sync_root_respects_exclude() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let sync_root = dir.path().join("new-root");
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1/keep\" \"$1/excluded\" && printf included > \"$1/keep/file.txt\" && printf excluded > \"$1/excluded/file.txt\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(
		&[
			"pull",
			"--sync-root",
			"new-root",
			"--remote-dir",
			&remote_proj_dir,
			"--exclude",
			"new-root/excluded/**",
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert_eq!(
		fs::read_to_string(sync_root.join("keep/file.txt"))?,
		"included"
	);
	assert!(!sync_root.join("excluded/file.txt").exists());

	Ok(())
}

#[test]
fn e2e_pull_rejects_remote_symlink() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let setup_output = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && ln -s /tmp \"$1/link\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup_output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup_output.stderr)
	);

	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("Refusing to pull remote symlink entries: link"),
		"stderr: {stderr}"
	);
	assert!(!dir.path().join("link").exists());

	Ok(())
}

#[test]
fn e2e_sync_never_transfers_or_deletes_git_metadata() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	fs::write(dir.path().join(".git"), "gitdir: /safe/local\n")?;
	fs::create_dir_all(dir.path().join("nested/.git"))?;
	fs::write(dir.path().join("nested/.git/config"), "local metadata")?;
	let setup_dir = tempfile::tempdir()?;

	let setup = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1/nested/.git\" && chmod 751 \"$1/nested/.git\" && printf 'gitdir: /safe/remote\\n' > \"$1/.git\" && printf 'remote metadata' > \"$1/nested/.git/config\" && printf result > \"$1/result.txt\"",
			"sh",
			&remote_dir,
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup.stderr)
	);

	let pull = biwa_cmd_tilde(&["pull", "--remote-dir", &remote_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		pull.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&pull.stderr)
	);
	assert_eq!(
		fs::read_to_string(dir.path().join(".git"))?,
		"gitdir: /safe/local\n"
	);
	assert_eq!(
		fs::read_to_string(dir.path().join("nested/.git/config"))?,
		"local metadata"
	);
	assert_eq!(fs::read_to_string(dir.path().join("result.txt"))?, "result");

	fs::write(dir.path().join("ordinary.txt"), "upload")?;
	let push = biwa_cmd_tilde(&["sync", "--remote-dir", &remote_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		push.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&push.stderr)
	);

	let check = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"-d",
			&remote_dir,
			"sh",
			"-c",
			"test \"$(cat .git)\" = 'gitdir: /safe/remote' && test \"$(cat nested/.git/config)\" = 'remote metadata' && test \"$(stat -c %a nested/.git)\" = 751 && test \"$(cat ordinary.txt)\" = upload",
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		check.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&check.stderr)
	);
	Ok(())
}

#[cfg(unix)]
fn run_pull_signal_during_commit(signal: Signal) -> Result<()> {
	use nix::sys::signal::kill;
	use nix::unistd::Pid;
	use std::io::Result as IoResult;
	use std::process::{Command, Stdio};
	use std::thread;
	use std::time::{Duration, Instant};

	const FILE_COUNT: usize = 500;
	let dir = tempfile::tempdir()?;
	let state_dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	for index in 0..FILE_COUNT {
		fs::write(
			dir.path().join(format!("file-{index:04}.txt")),
			format!("local-{index:04}"),
		)?;
	}

	let initial = biwa_cmd_tilde(&["sync", "--remote-dir", &remote_dir], dir.path())
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", FILE_COUNT.to_string())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		initial.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&initial.stderr)
	);
	let update = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"-d",
			&remote_dir,
			"sh",
			"-c",
			"for file in ./*.txt; do printf remote > \"$file\"; done",
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		update.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&update.stderr)
	);

	let mut command = Command::new(env!("CARGO_BIN_EXE_biwa"));
	command
		.args(["pull", "--remote-dir", &remote_dir])
		.current_dir(dir.path())
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", ssh_port())
		.env("BIWA_SSH_USER", "testuser")
		.env("BIWA_SSH_AUTH", "password")
		.env("BIWA_SSH_PASSWORD", "password123")
		.env("BIWA_SSH_HOST_KEY_CHECKING", "accept-new")
		.env("BIWA_SSH_KNOWN_HOSTS", common::test_known_hosts_path())
		.env("BIWA_SYNC_REMOTE_ROOT", "~/.cache/biwa/projects")
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", FILE_COUNT.to_string())
		.env("BIWA_CLEAN_AUTO", "false")
		.env("BIWA_STATE_DIR", state_dir.path())
		.stdout(Stdio::null())
		.stderr(Stdio::piped());
	let mut child = command.spawn()?;
	let deadline = Instant::now()
		.checked_add(Duration::from_secs(20))
		.ok_or_else(|| eyre!("pull commit deadline overflowed"))?;
	loop {
		let commit_started = fs::read_dir(dir.path())?
			.filter_map(IoResult::ok)
			.any(|entry| {
				entry
					.file_name()
					.to_string_lossy()
					.starts_with(".biwa-pull-stage-")
					&& entry.path().join("backups").is_dir()
			});
		if commit_started {
			let pid = i32::try_from(child.id())?;
			kill(Pid::from_raw(pid), signal)?;
			break;
		}
		if let Some(status) = child.try_wait()? {
			return Err(eyre!(
				"pull exited before its commit could be interrupted: {status}"
			));
		}
		if Instant::now() >= deadline {
			child.kill()?;
			return Err(eyre!("timed out waiting for pull commit to begin"));
		}
		thread::sleep(Duration::from_millis(1));
	}

	let output = child.wait_with_output()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("interrupted"), "stderr: {stderr}");
	assert!(stderr.contains("rolled back"), "stderr: {stderr}");
	for index in 0..FILE_COUNT {
		assert_eq!(
			fs::read_to_string(dir.path().join(format!("file-{index:04}.txt")))?,
			format!("local-{index:04}")
		);
	}
	assert!(
		!fs::read_dir(dir.path())?
			.filter_map(IoResult::ok)
			.any(|entry| entry
				.file_name()
				.to_string_lossy()
				.starts_with(".biwa-pull-stage-")),
		"pull staging directory remained after rollback"
	);
	Ok(())
}

#[cfg(unix)]
#[test]
fn e2e_pull_sigterm_during_commit_rolls_back_local_tree() -> Result<()> {
	run_pull_signal_during_commit(Signal::SIGTERM)
}

#[cfg(unix)]
#[test]
fn e2e_pull_sighup_during_commit_rolls_back_local_tree() -> Result<()> {
	run_pull_signal_during_commit(Signal::SIGHUP)
}

#[test]
fn e2e_pull_rejects_remote_root_symlink_with_trailing_component() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("sentinel.txt"), "local")?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	let target_dir = format!("{remote_dir}_target");
	let setup_dir = tempfile::tempdir()?;

	let setup = biwa_cmd_tilde(
		&[
			"run",
			"-d",
			"~",
			"sh",
			"-c",
			"mkdir -p \"$1\" && printf remote > \"$1/remote.txt\" && ln -s \"$1\" \"$2\"",
			"sh",
			&target_dir,
			&remote_dir,
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		setup.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&setup.stderr)
	);

	let remote_with_suffix = format!("{remote_dir}/.");
	let output = biwa_cmd_tilde(&["pull", "--remote-dir", &remote_with_suffix], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("remote directory is a symlink"),
		"stderr: {stderr}"
	);
	assert_eq!(
		fs::read_to_string(dir.path().join("sentinel.txt"))?,
		"local"
	);
	assert!(!dir.path().join("remote.txt").exists());
	Ok(())
}

#[test]
fn e2e_sync_absolute_path() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("absolute.txt"), "hello absolute")?;

	// Explicit override of BIWA_SYNC_REMOTE_ROOT to an absolute path
	let output = biwa_cmd(&["sync"], dir.path())
		.env("BIWA_SYNC_REMOTE_ROOT", "/tmp/biwa_test_absolute")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"First sync failed: stderr: {stderr}"
	);
	assert!(
		stderr.contains("1 uploaded"),
		"First sync didn't upload: stderr: {stderr}"
	);

	// Run command and explicitly assert on the absolute path
	let proj_name_full = common::get_remote_project_dir(dir.path())?;
	let proj_name_suffix = proj_name_full
		.strip_prefix("~/.cache/biwa/projects/")
		.unwrap_or(&proj_name_full);

	let remote_file = format!("/tmp/biwa_test_absolute/{proj_name_suffix}/absolute.txt");
	let output2 = biwa_cmd(&["run", "cat", &remote_file], dir.path())
		.env("BIWA_SYNC_REMOTE_ROOT", "/tmp/biwa_test_absolute")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stdout2 = String::from_utf8_lossy(&output2.stdout);
	assert!(output2.status.success(), "cat failed for {remote_file}");
	assert!(stdout2.contains("hello absolute"));

	// Cleanup the absolute directory to be a good citizen
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is acceptable")]
	biwa_cmd(&["run", "rm", "-rf", "/tmp/biwa_test_absolute"], dir.path())
		.run()
		.ok();

	Ok(())
}

#[test]
fn e2e_sync_cleaning() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("to_delete.txt");
	fs::write(&file_path, "delete me")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"));

	fs::remove_file(&file_path)?;

	let output2 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr2 = String::from_utf8_lossy(&output2.stderr);
	assert!(stderr2.contains("1 deleted"));
	Ok(())
}

#[test]
fn e2e_sync_empty_dir_created() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("empty"))?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_empty = format!("{remote_proj_dir}/empty");
	let check_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test -d \"$1\"",
			"sh",
			&remote_empty,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		check_output.status.success(),
		"remote dir missing: {remote_empty}"
	);

	Ok(())
}

#[test]
fn e2e_sync_empty_dir_removed() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let empty_dir = dir.path().join("empty");
	fs::create_dir_all(&empty_dir)?;

	let first_sync = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		first_sync.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&first_sync.stderr)
	);

	fs::remove_dir(&empty_dir)?;

	let second_sync = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		second_sync.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&second_sync.stderr)
	);

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_empty = format!("{remote_proj_dir}/empty");
	let check_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&remote_empty,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		check_output.status.success(),
		"remote dir still exists: {remote_empty}"
	);

	Ok(())
}

#[test]
fn e2e_sync_preserves_dir_when_last_file_deleted() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let nested_dir = dir.path().join("dir");
	fs::create_dir_all(&nested_dir)?;
	let file_path = nested_dir.join("file.txt");
	fs::write(&file_path, "hello")?;

	let first_sync = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		first_sync.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&first_sync.stderr)
	);

	fs::remove_file(&file_path)?;

	let second_sync = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&second_sync.stderr);
	assert!(second_sync.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 deleted"), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_dir = format!("{remote_proj_dir}/dir");
	let remote_file = format!("{remote_dir}/file.txt");

	let dir_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test -d \"$1\"",
			"sh",
			&remote_dir,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		dir_output.status.success(),
		"remote dir missing: {remote_dir}"
	);

	let file_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&remote_file,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		file_output.status.success(),
		"remote file still exists: {remote_file}"
	);

	Ok(())
}

#[test]
fn e2e_sync_preserves_parent_dir_when_nested_dir_removed() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let nested_dir = dir.path().join("a").join("b");
	fs::create_dir_all(&nested_dir)?;

	let first_sync = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		first_sync.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&first_sync.stderr)
	);

	fs::remove_dir(&nested_dir)?;

	let second_sync = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&second_sync.stderr);
	assert!(second_sync.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 deleted"), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_parent = format!("{remote_proj_dir}/a");
	let remote_nested = format!("{remote_proj_dir}/a/b");

	let parent_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test -d \"$1\"",
			"sh",
			&remote_parent,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		parent_output.status.success(),
		"remote parent dir missing: {remote_parent}"
	);

	let nested_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&remote_nested,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		nested_output.status.success(),
		"remote nested dir still exists: {remote_nested}"
	);

	Ok(())
}

#[rstest]
#[case::default(None, "drwx------", "-rw-------", "-rwx------", "-rw-------")]
#[case::umask_0077(Some("0077"), "drwx------", "-rw-------", "-rwx------", "-rw-------")]
#[case::umask_0022(Some("0022"), "drwxr-xr-x", "-rw-r--r--", "-rwxr-xr-x", "-rw-r--r--")]
#[case::umask_0027(Some("0027"), "drwxr-x---", "-rw-r-----", "-rwxr-x---", "-rw-r-----")]
fn e2e_sync_permissions(
	#[case] umask: Option<&str>,
	#[case] expected_dir: &str,
	#[case] expected_secret: &str,
	#[case] expected_script: &str,
	#[case] expected_group: &str,
) -> Result<()> {
	let dir = tempfile::tempdir()?;
	let dir_path = dir.path().join("subdir");
	fs::create_dir_all(&dir_path)?;

	let secret_path = dir_path.join("secret.txt");
	fs::write(&secret_path, "secret")?;

	// Create an executable file
	let script_path = dir_path.join("script.sh");
	fs::write(&script_path, "#!/bin/sh\necho hi")?;

	let group_path = dir_path.join("group.txt");
	fs::write(&group_path, "group")?;

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;

		// 0775 for subdir
		let mut perms = fs::metadata(&dir_path)?.permissions();
		perms.set_mode(0o775);
		fs::set_permissions(&dir_path, perms)?;

		// 0644 for secret.txt (to verify permissive umask doesn't add perms)
		let mut perms = fs::metadata(&secret_path)?.permissions();
		perms.set_mode(0o644);
		fs::set_permissions(&secret_path, perms)?;

		// 0755 for script.sh
		let mut perms = fs::metadata(&script_path)?.permissions();
		perms.set_mode(0o755);
		fs::set_permissions(&script_path, perms)?;

		// 0664 for group.txt
		let mut perms = fs::metadata(&group_path)?.permissions();
		perms.set_mode(0o664);
		fs::set_permissions(&group_path, perms)?;
	}

	let run_cmd = |args: &[&str]| {
		let mut cmd = biwa_cmd_tilde(args, dir.path());
		if let Some(u) = umask {
			cmd = cmd.env("BIWA_SSH_UMASK", u);
		}
		cmd
	};

	let output = run_cmd(&["sync"])
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	assert!(
		output.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_dir = format!("{remote_proj_dir}/subdir");

	let ls_output = run_cmd(&["run", "ls", "-ld", &remote_dir])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(ls_stdout.contains(expected_dir), "dir stdout: {ls_stdout}");

	let remote_file = format!("{remote_dir}/secret.txt");
	let ls_file_output = run_cmd(&["run", "ls", "-l", &remote_file])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_file_stdout = String::from_utf8_lossy(&ls_file_output.stdout);
	assert!(
		ls_file_stdout.contains(expected_secret),
		"secret stdout: {ls_file_stdout}"
	);

	let remote_script = format!("{remote_dir}/script.sh");
	let ls_script_output = run_cmd(&["run", "ls", "-l", &remote_script])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_script_stdout = String::from_utf8_lossy(&ls_script_output.stdout);
	assert!(
		ls_script_stdout.contains(expected_script),
		"script stdout: {ls_script_stdout}"
	);

	let remote_group = format!("{remote_dir}/group.txt");
	let ls_group_output = run_cmd(&["run", "ls", "-l", &remote_group])
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let ls_group_stdout = String::from_utf8_lossy(&ls_group_output.stdout);
	assert!(
		ls_group_stdout.contains(expected_group),
		"group stdout: {ls_group_stdout}"
	);
	Ok(())
}

#[test]
fn e2e_sync_setstat_permissions_on_capable_server() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("secret.txt");
	fs::write(&file_path, "secret")?;

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;

		let mut perms = fs::metadata(&file_path)?.permissions();
		perms.set_mode(0o644);
		fs::set_permissions(&file_path, perms)?;
	}

	let output = biwa_cmd_capable_tilde(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_PERMISSIONS", "setstat")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(
		!stderr.contains("Failed to enforce file permissions via fsetstat"),
		"stderr: {stderr}"
	);

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_file = format!("{remote_proj_dir}/secret.txt");
	let ls_output = biwa_cmd_capable_tilde(
		&["run", "--skip-sync", "ls", "-l", &remote_file],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(
		ls_output.status.success(),
		"ls failed for {remote_file}: {ls_stdout}\nstderr: {}",
		String::from_utf8_lossy(&ls_output.stderr)
	);
	assert!(
		ls_stdout.contains("-rw-------"),
		"File {remote_file} does not have 0600 permissions. ls output: {ls_stdout}"
	);
	Ok(())
}

#[test]
fn e2e_sync_hashing() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hash.txt");
	fs::write(&file_path, "initial")?;

	let output1 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	fs::write(&file_path, "modified")?;

	let output2 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output2.stderr).contains("1 uploaded"));
	assert!(String::from_utf8_lossy(&output2.stderr).contains("0 unchanged"));

	// Unchanged
	let output3 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	assert!(String::from_utf8_lossy(&output3.stderr).contains("0 uploaded"));
	assert!(String::from_utf8_lossy(&output3.stderr).contains("1 unchanged"));
	Ok(())
}

#[test]
fn e2e_sync_abort() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("file1.txt"), "1")?;
	fs::write(dir.path().join("file2.txt"), "2")?;

	// Set max_files_to_sync to 1
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", "1")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains("Aborting synchronization: 2 files to sync exceeds the limit of 1."));
	Ok(())
}

#[test]
fn e2e_sync_abort_before_connecting() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("file1.txt"), "1")?;
	fs::write(dir.path().join("file2.txt"), "2")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", "1")
		.env("BIWA_SSH_HOST", "127.0.0.1")
		.env("BIWA_SSH_PORT", "1")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains("Aborting synchronization: 2 files to sync exceeds the limit of 1."));
	assert!(!stderr.contains("Failed to connect to"));
	Ok(())
}

#[test]
fn e2e_sync_ignore_gitignore() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
	fs::write(dir.path().join("ignored.txt"), "this should not sync")?;
	fs::write(dir.path().join("kept.txt"), "this should sync")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 uploaded"), "stderr: {stderr}"); // kept.txt and .gitignore
	Ok(())
}

#[test]
fn e2e_sync_ignore_biwaignore() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".biwaignore"), ".env\n")?;
	fs::write(dir.path().join(".env"), "SECRET=val")?;
	fs::write(dir.path().join("main.rs"), "fn main() {}")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 uploaded"), "stderr: {stderr}"); // main.rs and .biwaignore
	Ok(())
}

#[test]
fn e2e_sync_exclude_globset() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let tests_dir = dir.path().join("tests");
	fs::create_dir_all(&tests_dir)?;
	fs::write(tests_dir.join("a.txt"), "a")?;
	fs::write(dir.path().join("b.txt"), "b")?;

	// Exclude tests directory relative to current cwd
	let output = biwa_cmd_tilde(&["sync", "--exclude", "tests/**"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}"); // Only b.txt

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_tests_dir = format!("{remote_proj_dir}/tests");
	let check_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&remote_tests_dir,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		check_output.status.success(),
		"excluded dir was created: {remote_tests_dir}"
	);

	Ok(())
}

#[test]
fn e2e_sync_exclude_empty_dir() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::create_dir_all(dir.path().join("ignored"))?;

	let output = biwa_cmd_tilde(&["sync", "--exclude", "ignored"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_dir = format!("{remote_proj_dir}/ignored");
	let check_output = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&remote_dir,
		],
		dir.path(),
	)
	.unchecked()
	.run()?;
	assert!(
		check_output.status.success(),
		"excluded dir was created: {remote_dir}"
	);

	Ok(())
}

#[test]
fn e2e_sync_exclude_config() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(
		dir.path().join("biwa.toml"),
		"sync.exclude = [\"secret_*.txt\"]\n",
	)?;
	fs::write(dir.path().join("secret_a.txt"), "a")?;
	fs::write(dir.path().join("public.txt"), "b")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("2 uploaded"), "stderr: {stderr}"); // biwa.toml and public.txt
	Ok(())
}

#[test]
fn e2e_sync_force() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("file.txt"), "content")?;

	let output1 = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;
	assert!(String::from_utf8_lossy(&output1.stderr).contains("1 uploaded"));

	let output2 = biwa_cmd_tilde(&["sync", "--force"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stderr2 = String::from_utf8_lossy(&output2.stderr);
	assert!(stderr2.contains("1 uploaded"), "stderr2: {stderr2}");
	assert!(stderr2.contains("0 unchanged"), "stderr2: {stderr2}");
	Ok(())
}

#[test]
fn e2e_sync_large_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	// 1MB file
	let large_content = vec![b'a'; 1024 * 1024];
	fs::write(dir.path().join("large.bin"), &large_content)?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success());
	assert!(stderr.contains("1 uploaded"));
	Ok(())
}

#[test]
fn e2e_sync_remote_symlink() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;

	// Create a secondary dummy project just to run setup commands without BIWA trying to CD
	// into `remote_dir` (which doesn't exist or is a symlink we are trying to create).
	let setup_dir = tempfile::tempdir()?;

	// Create a dummy dir to point the symlink to
	let dummy_dir = format!("{remote_dir}_dummy");
	biwa_cmd_tilde(
		&[
			"run",
			"sh",
			"-c",
			&format!("mkdir -p {dummy_dir} && ln -s {dummy_dir} {remote_dir}"),
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;

	// Now try to run sync, it should fail
	fs::write(dir.path().join("test.txt"), "test")?;
	let remote_with_slash = format!("{remote_dir}/");
	let output = biwa_cmd_tilde(&["sync", "--remote-dir", &remote_with_slash], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		!output.status.success(),
		"Expected failure but succeeded.\nstdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		stderr
	);
	assert!(
		stderr.contains("remote directory is a symlink"),
		"stderr: {stderr}"
	);

	Ok(())
}

#[test]
fn e2e_sync_shell_injection() -> Result<()> {
	let base_dir = tempfile::tempdir()?;
	let malicious_name = "test_dir_$(echo injection_attempt)_'\"`";
	let proj_dir = base_dir.path().join(malicious_name);
	fs::create_dir_all(&proj_dir)?;
	fs::write(proj_dir.join("test.txt"), "content")?;

	// Sync should work correctly despite the malicious project name
	let output = biwa_cmd_tilde(&["sync"], &proj_dir)
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Compute unique project name
	let remote_proj_dir = common::get_remote_project_dir(&proj_dir)?;
	let remote_file = format!("{remote_proj_dir}/test.txt");

	let output_cat = biwa_cmd_tilde(&["run", "cat", &remote_file], &proj_dir)
		.stdout_capture()
		.stderr_capture()
		.run()?;

	let stdout_cat = String::from_utf8_lossy(&output_cat.stdout);
	// BIWA CLI warnings might be logged to stdout in some configurations, so we check if it ends with or contains our expected text.
	assert!(
		stdout_cat.trim().ends_with("content"),
		"stdout: {stdout_cat}"
	);

	Ok(())
}

#[test]
fn e2e_sync_intermediate_dir_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let deep_dir = dir.path().join("a").join("b").join("c");
	fs::create_dir_all(&deep_dir)?;
	fs::write(deep_dir.join("file.txt"), "hello")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	for path in ["", "/a", "/a/b", "/a/b/c"] {
		let remote_path = format!("{remote_proj_dir}{path}");
		let ls_output = biwa_cmd_tilde(&["run", "ls", "-ld", &remote_path], dir.path())
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()?;

		let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
		assert!(
			ls_output.status.success(),
			"ls failed for {remote_path}: {ls_stdout}\nstderr: {}",
			String::from_utf8_lossy(&ls_output.stderr)
		);
		assert!(
			ls_stdout.contains("drwx------"),
			"Directory {remote_path} does not have 0700 permissions. ls output: {ls_stdout}"
		);
	}

	Ok(())
}

#[test]
fn e2e_sync_empty_dir_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let deep_dir = dir.path().join("a").join("b").join("c");
	fs::create_dir_all(&deep_dir)?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	for path in ["", "/a", "/a/b", "/a/b/c"] {
		let remote_path = format!("{remote_proj_dir}{path}");
		let ls_output = biwa_cmd_tilde(
			&["run", "--skip-sync", "ls", "-ld", &remote_path],
			dir.path(),
		)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

		let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
		assert!(
			ls_output.status.success(),
			"ls failed for {remote_path}: {ls_stdout}\nstderr: {}",
			String::from_utf8_lossy(&ls_output.stderr)
		);
		assert!(
			ls_stdout.contains("drwx------"),
			"Directory {remote_path} does not have 0700 permissions. ls output: {ls_stdout}"
		);
	}

	Ok(())
}

#[test]
fn e2e_sync_existing_dir_permissions() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let sub_dir = dir.path().join("preexisting");
	fs::create_dir_all(&sub_dir)?;
	fs::write(sub_dir.join("file.txt"), "hello")?;

	// 1. Manually create the remote directory with 0755 permissions
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let remote_sub_dir = format!("{remote_proj_dir}/preexisting");

	// Ensure the base directory is created first and then the sub_dir with 0755
	biwa_cmd_tilde(
		&[
			"run",
			"sh",
			"-c",
			&format!("mkdir -p {remote_proj_dir} && mkdir -m 0755 {remote_sub_dir}"),
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	// 2. Sync the project
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");

	// 3. Verify that the permissions of the pre-existing directory were corrected to 0700
	let ls_output = biwa_cmd_tilde(&["run", "ls", "-ld", &remote_sub_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
	assert!(
		ls_output.status.success(),
		"ls failed for {remote_sub_dir}: {ls_stdout}\nstderr: {}",
		String::from_utf8_lossy(&ls_output.stderr)
	);
	assert!(
		ls_stdout.contains("drwx------"),
		"Pre-existing directory {remote_sub_dir} was not corrected to 0700 permissions. ls output: {ls_stdout}"
	);

	// 4. Verify the project root itself was corrected to 0700
	let ls_root_output = biwa_cmd_tilde(&["run", "ls", "-ld", &remote_proj_dir], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let ls_root_stdout = String::from_utf8_lossy(&ls_root_output.stdout);
	assert!(
		ls_root_stdout.contains("drwx------"),
		"Project root {remote_proj_dir} was not corrected to 0700 permissions. ls output: {ls_root_stdout}"
	);

	Ok(())
}

#[test]
fn e2e_sync_remote_dir() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "remote dir test")?;

	let test_id = dir
		.path()
		.file_name()
		.ok_or_else(|| eyre!("Failed to get test ID from path: {:?}", dir.path()))?
		.to_string_lossy();
	let remote_dir_path_string = format!("/tmp/biwa_test_remote_dir_{test_id}");
	let remote_dir_path = remote_dir_path_string.as_str();
	let output = biwa_cmd(&["sync", "-d", remote_dir_path], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	let output_cat = biwa_cmd(
		&["run", "-d", remote_dir_path, "cat", "hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout_cat = String::from_utf8_lossy(&output_cat.stdout);
	assert!(
		output_cat.status.success(),
		"cat failed, stderr: {}",
		String::from_utf8_lossy(&output_cat.stderr)
	);
	assert!(stdout_cat.contains("remote dir test"));

	// Cleanup
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is acceptable")]
	biwa_cmd(
		&["run", "--skip-sync", "rm", "-rf", remote_dir_path],
		dir.path(),
	)
	.run()
	.ok();

	Ok(())
}

#[test]
fn e2e_sync_remote_dir_tilde() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "tilde test")?;

	let test_id = dir
		.path()
		.file_name()
		.ok_or_else(|| eyre!("Failed to get test ID from path: {:?}", dir.path()))?
		.to_string_lossy();
	let remote_dir_path_string = format!("~/biwa_test_tilde_{test_id}");
	let remote_dir_path = remote_dir_path_string.as_str();
	let output = biwa_cmd(&["sync", "-d", remote_dir_path], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	let output_cat = biwa_cmd(
		&["run", "-d", remote_dir_path, "cat", "hello.txt"],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;

	let stdout_cat = String::from_utf8_lossy(&output_cat.stdout);
	assert!(
		output_cat.status.success(),
		"cat failed, stderr: {}",
		String::from_utf8_lossy(&output_cat.stderr)
	);
	assert!(stdout_cat.contains("tilde test"));

	// Check that we didn't create a literal "~" directory
	let output_test = biwa_cmd(&["run", "--skip-sync", "test", "-d", "./~"], dir.path())
		.unchecked()
		.run()?;
	assert!(
		!output_test.status.success(),
		"Literal ~ directory was created!"
	);

	// Cleanup
	#[expect(clippy::unused_result_ok, reason = "Cleanup failure is acceptable")]
	biwa_cmd(
		&["run", "--skip-sync", "rm", "-rf", remote_dir_path],
		dir.path(),
	)
	.run()
	.ok();

	Ok(())
}

#[test]
fn e2e_sync_remote_file_symlink_overwrite() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;

	// Create a secondary dummy project just to run setup commands without BIWA trying to CD
	// into `remote_dir`
	let setup_dir = tempfile::tempdir()?;

	// Create a dummy dir to point the file symlink to
	let dummy_dir = format!("{remote_dir}_dummy");
	biwa_cmd_tilde(
		&[
			"run",
			"sh",
			"-c",
			&format!(
				"mkdir -p {dummy_dir} && echo 'secret' > {dummy_dir}/sensitive.txt && \
				 mkdir -p {remote_dir} && ln -s {dummy_dir}/sensitive.txt {remote_dir}/test.txt"
			),
		],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;

	// Now try to run sync, it should succeed and replace the file symlink
	fs::write(dir.path().join("test.txt"), "overwritten")?;
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success(),
		"Expected success but failed.\nstdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		stderr
	);
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// Verify the original sensitive file was not overwritten
	let output_sensitive = biwa_cmd_tilde(
		&["run", "cat", &format!("{dummy_dir}/sensitive.txt")],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;
	let stdout_sensitive = String::from_utf8_lossy(&output_sensitive.stdout);
	assert!(
		stdout_sensitive.contains("secret"),
		"Sensitive file was overwritten or missing!"
	);

	// Verify the synced file is correct
	let output_synced = biwa_cmd_tilde(
		&["run", "cat", &format!("{remote_dir}/test.txt")],
		setup_dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.run()?;
	let stdout_synced = String::from_utf8_lossy(&output_synced.stdout);
	assert!(
		stdout_synced.contains("overwritten"),
		"Synced file content is incorrect: {stdout_synced}"
	);

	Ok(())
}

#[test]
fn e2e_sync_hidden_file() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join(".secret_config"), "my secret config")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	Ok(())
}

/// Builds a `biwa sync` command with the sync cache stored in an explicit directory.
fn cached_sync_cmd(project_dir: &Path, cache_dir: &Path, args: &[&str]) -> duct::Expression {
	let mut full_args = vec!["sync"];
	full_args.extend_from_slice(args);
	biwa_cmd_tilde(&full_args, project_dir)
		.env("BIWA_SYNC_SFTP_CACHE_PATH", cache_dir)
		.stdout_capture()
		.stderr_capture()
		.unchecked()
}

/// Runs a prepared `biwa sync` command and returns its stderr.
fn run_sync_cmd(command: &duct::Expression) -> Result<String> {
	let output = command.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
	if !output.status.success() {
		return Err(eyre!("sync failed: {stderr}"));
	}
	Ok(stderr)
}

/// Runs `biwa sync` with the sync cache stored in an explicit directory.
fn run_cached_sync(project_dir: &Path, cache_dir: &Path, args: &[&str]) -> Result<String> {
	run_sync_cmd(&cached_sync_cmd(project_dir, cache_dir, args))
}

/// Runs a shell command inside the project's remote directory.
fn run_in_remote_dir(project_dir: &Path, remote_dir: &str, script: &str) -> Result<()> {
	let output = biwa_cmd_tilde(
		&["run", "--skip-sync", "-d", remote_dir, "sh", "-c", script],
		project_dir,
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	if !output.status.success() {
		return Err(eyre!(
			"remote command failed: {}",
			String::from_utf8_lossy(&output.stderr)
		));
	}
	Ok(())
}

/// Returns the only sync cache file in a cache directory.
fn sync_cache_file(cache_dir: &Path) -> Result<PathBuf> {
	let cache_files = fs::read_dir(cache_dir)?
		.map(|entry| Ok(entry?.path()))
		.collect::<Result<Vec<_>>>()?;
	let [cache_file] = cache_files.as_slice() else {
		return Err(eyre!("expected exactly one cache file: {cache_files:?}"));
	};
	Ok(cache_file.clone())
}

/// Returns the cached remote hash recorded for one relative path.
fn cached_remote_hash<'cache>(cache: &'cache serde_json::Value, path: &str) -> Option<&'cache str> {
	cache.get("remote_files")?.get(path)?.get("hash")?.as_str()
}

/// Reads the sync cache file as JSON.
fn read_sync_cache(cache_dir: &Path) -> Result<serde_json::Value> {
	Ok(serde_json::from_str(&fs::read_to_string(
		sync_cache_file(cache_dir)?,
	)?)?)
}

/// Returns the recorded time of the last full remote hash pass.
fn remote_hashed_at(cache: &serde_json::Value) -> Option<u64> {
	cache.get("remote_hashed_at")?.as_u64()
}

/// Returns the current Unix time in whole seconds.
fn unix_now() -> Result<u64> {
	Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// Rewrites the sync cache file after applying an edit to its JSON.
fn edit_sync_cache(
	cache_dir: &Path,
	edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<()>,
) -> Result<()> {
	let mut value = read_sync_cache(cache_dir)?;
	edit(
		value
			.as_object_mut()
			.ok_or_else(|| eyre!("sync cache is not an object"))?,
	)?;
	fs::write(sync_cache_file(cache_dir)?, serde_json::to_string(&value)?)?;
	Ok(())
}

/// Replaces the hash recorded for one cached remote file.
fn set_cached_remote_hash(cache_dir: &Path, path: &str, hash: &str) -> Result<()> {
	edit_sync_cache(cache_dir, |cache| {
		cache
			.get_mut("remote_files")
			.and_then(|files| files.get_mut(path))
			.and_then(|entry| entry.as_object_mut())
			.ok_or_else(|| eyre!("missing cached remote entry for {path}"))?
			.insert("hash".to_owned(), serde_json::json!(hash));
		Ok(())
	})
}

/// Replaces a cached remote hash with a value no real file can have.
///
/// A sync that trusts the cache then sees the remote file as different from the
/// local one; a sync that re-hashes the remote directory does not.
fn poison_remote_cache(cache_dir: &Path, path: &str) -> Result<()> {
	set_cached_remote_hash(cache_dir, path, &"0".repeat(64))
}

/// Overwrites the recorded time of the last full remote hash pass.
fn set_remote_hashed_at(cache_dir: &Path, seconds: u64) -> Result<()> {
	edit_sync_cache(cache_dir, |cache| {
		cache.insert("remote_hashed_at".to_owned(), serde_json::json!(seconds));
		Ok(())
	})
}

/// Returns the lowercase hex SHA-256 digest of some content.
fn sha256_hex(content: &str) -> String {
	hex::encode(<sha2::Sha256 as sha2::Digest>::digest(content.as_bytes()))
}

/// The window biwa requires a remote timestamp to be outside of before caching.
///
/// Mirrors `RACY_MTIME_WINDOW_SECS` in `src/ssh/sync_cache.rs`, which is private
/// to the crate; keep the two in step.
const REMOTE_SETTLE_WINDOW: Duration = Duration::from_secs(2);

/// Extra time covering fractional timestamps against a whole-second remote clock.
const REMOTE_SETTLE_HEADROOM: Duration = Duration::from_secs(1);

/// Waits until remote timestamps written just now can key a cache entry.
///
/// A remote fingerprint is only recorded once its modification *and* change
/// times are outside [`REMOTE_SETTLE_WINDOW`], which guards against a rewrite
/// reproducing a fingerprint. Change times cannot be back-dated from user
/// space, so the window has to be waited out.
fn wait_for_settled_remote_timestamps() {
	sleep(REMOTE_SETTLE_WINDOW.saturating_add(REMOTE_SETTLE_HEADROOM));
}

/// Syncs a fresh project twice so the sync cache holds settled remote state.
///
/// The first sync uploads the file, whose remote copy is written too recently
/// to be fingerprinted; the second sync, run after the window, records it.
fn prime_remote_cache(project_dir: &Path, cache_dir: &Path) -> Result<()> {
	let stderr = run_cached_sync(project_dir, cache_dir, &[])?;
	if !stderr.contains("1 uploaded") {
		return Err(eyre!("expected the first sync to upload: {stderr}"));
	}
	wait_for_settled_remote_timestamps();
	let stderr = run_cached_sync(project_dir, cache_dir, &[])?;
	if !stderr.contains("0 uploaded") {
		return Err(eyre!(
			"expected the second sync to upload nothing: {stderr}"
		));
	}
	let cache = read_sync_cache(cache_dir)?;
	if cached_remote_hash(&cache, "hello.txt").is_none() {
		return Err(eyre!("remote hash was not cached: {cache}"));
	}
	Ok(())
}

/// Moves a file's modification time into the past so its hash is cacheable.
fn backdate_mtime(path: &Path) -> Result<()> {
	let file = fs::File::options().append(true).open(path)?;
	let mtime = SystemTime::now()
		.checked_sub(Duration::from_secs(60))
		.ok_or_else(|| eyre!("failed to compute past mtime"))?;
	file.set_modified(mtime)?;
	Ok(())
}

#[test]
fn e2e_sync_cache_reuses_local_hashes() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;

	// First sync uploads the file and writes a cache file.
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	let cache_files = fs::read_dir(cache_dir.path())?
		.map(|entry| Ok(entry?.path()))
		.collect::<Result<Vec<_>>>()?;
	let [cache_file] = cache_files.as_slice() else {
		return Err(eyre!("expected exactly one cache file: {cache_files:?}"));
	};
	assert!(fs::read_to_string(cache_file)?.contains("hello.txt"));

	// Second sync reuses the cache and uploads nothing.
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	assert!(stderr.contains("1 unchanged"), "stderr: {stderr}");

	// Poison the cached hash. The unchanged file now looks modified, proving
	// the cached hash is consulted instead of re-hashing the content.
	let mut value: serde_json::Value = serde_json::from_str(&fs::read_to_string(cache_file)?)?;
	value
		.get_mut("files")
		.and_then(|files| files.get_mut("hello.txt"))
		.and_then(|entry| entry.as_object_mut())
		.ok_or_else(|| eyre!("missing cached hello.txt entry"))?
		.insert("hash".to_owned(), serde_json::json!("0".repeat(64)));
	fs::write(cache_file, serde_json::to_string(&value)?)?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// --force bypasses and rebuilds the cache with correct hashes.
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &["--force"])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	Ok(())
}

#[test]
fn e2e_sync_cache_disabled_writes_no_cache() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "world")?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_CACHE_PATH", cache_dir.path())
		.env("BIWA_SYNC_SFTP_CACHE_ENABLED", "false")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	assert!(fs::read_dir(cache_dir.path())?.next().is_none());
	Ok(())
}

#[test]
fn e2e_sync_cache_reuses_remote_hashes() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;
	prime_remote_cache(dir.path(), cache_dir.path())?;

	// Poison the cached remote hash. The untouched remote file now looks
	// different from the local one, proving the remote inventory reused the
	// cached hash instead of running `sha256sum` again.
	poison_remote_cache(cache_dir.path(), "hello.txt")?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// The upload invalidated the entry, so the next run hashes it again and
	// records the real hash.
	wait_for_settled_remote_timestamps();
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	let cache = read_sync_cache(cache_dir.path())?;
	assert_ne!(
		cached_remote_hash(&cache, "hello.txt"),
		Some("0".repeat(64).as_str()),
		"cache: {cache}"
	);
	Ok(())
}

#[test]
fn e2e_sync_cache_detects_remote_drift() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;
	prime_remote_cache(dir.path(), cache_dir.path())?;

	// Rewriting the remote file outside biwa moves its size, modification time,
	// and change time, so the cached hash is rejected and the file is hashed
	// again.
	run_in_remote_dir(dir.path(), &remote_dir, "printf 'drifted away' > hello.txt")?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	run_in_remote_dir(dir.path(), &remote_dir, "test \"$(cat hello.txt)\" = world")?;
	Ok(())
}

#[test]
fn e2e_sync_cache_revalidates_remote_hashes() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;
	prime_remote_cache(dir.path(), cache_dir.path())?;

	// An expired full hash pass re-hashes the whole remote directory, so the
	// poisoned entry is ignored and the matching file is left alone.
	poison_remote_cache(cache_dir.path(), "hello.txt")?;
	set_remote_hashed_at(cache_dir.path(), 1)?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");

	// With automatic revalidation disabled the cached hash is trusted until its
	// fingerprint changes, even long after the last full pass.
	poison_remote_cache(cache_dir.path(), "hello.txt")?;
	set_remote_hashed_at(cache_dir.path(), 1)?;
	let stderr = run_sync_cmd(
		&cached_sync_cmd(dir.path(), cache_dir.path(), &[])
			.env("BIWA_SYNC_SFTP_CACHE_AUTO_REVALIDATE", "false"),
	)?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	Ok(())
}

#[test]
fn e2e_sync_force_rebuilds_the_remote_cache() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;
	prime_remote_cache(dir.path(), cache_dir.path())?;

	// An hour-old full pass is not due for revalidation, so an ordinary sync
	// reuses the cached hashes and leaves the recorded time alone.
	let recent = unix_now()?.saturating_sub(3600);
	set_remote_hashed_at(cache_dir.path(), recent)?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	let cache = read_sync_cache(cache_dir.path())?;
	assert_eq!(remote_hashed_at(&cache), Some(recent), "cache: {cache}");

	// --force ignores the poisoned cache and hashes the whole remote directory
	// again, which is what moves the recorded time forward.
	poison_remote_cache(cache_dir.path(), "hello.txt")?;
	set_remote_hashed_at(cache_dir.path(), recent)?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &["--force"])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");
	let cache = read_sync_cache(cache_dir.path())?;
	assert_ne!(remote_hashed_at(&cache), Some(recent), "cache: {cache}");

	// The rebuilt cache holds the real hash rather than the poisoned one.
	wait_for_settled_remote_timestamps();
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	let cache = read_sync_cache(cache_dir.path())?;
	assert_ne!(
		cached_remote_hash(&cache, "hello.txt"),
		Some("0".repeat(64).as_str()),
		"cache: {cache}"
	);
	Ok(())
}

#[test]
fn e2e_sync_handles_pre_epoch_remote_timestamps() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("1 uploaded"), "stderr: {stderr}");

	// GNU find prints a leading sign for a pre-epoch timestamp, which cannot key
	// a cache entry. The file must still sync, and must not look remotely
	// deleted: a re-upload would show up as another uploaded file.
	run_in_remote_dir(dir.path(), &remote_dir, "touch -t 196001011200 hello.txt")?;
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	assert!(stderr.contains("1 unchanged"), "stderr: {stderr}");
	let cache = read_sync_cache(cache_dir.path())?;
	assert_eq!(
		cached_remote_hash(&cache, "hello.txt"),
		None,
		"cache: {cache}"
	);

	// It is simply hashed again on every later run.
	let stderr = run_cached_sync(dir.path(), cache_dir.path(), &[])?;
	assert!(stderr.contains("0 uploaded"), "stderr: {stderr}");
	assert!(stderr.contains("1 unchanged"), "stderr: {stderr}");
	Ok(())
}

#[test]
fn e2e_pull_verification_rejects_a_stale_cached_remote_hash() -> Result<()> {
	let dir = tempfile::tempdir()?;
	let cache_dir = tempfile::tempdir()?;
	let remote_dir = common::get_remote_project_dir(dir.path())?;
	let file_path = dir.path().join("hello.txt");
	fs::write(&file_path, "world")?;
	backdate_mtime(&file_path)?;
	prime_remote_cache(dir.path(), cache_dir.path())?;

	// Give the pull something to do, diverge the local file, and rewrite the
	// cached remote hash to claim the two sides already match.
	run_in_remote_dir(dir.path(), &remote_dir, "printf extra > extra.txt")?;
	fs::write(&file_path, "local")?;
	set_cached_remote_hash(cache_dir.path(), "hello.txt", &sha256_hex("local"))?;

	// Planning trusts the cache and skips hello.txt, but the verification pass
	// hashes the remote directory again with the cache disabled and aborts
	// before any local file is touched.
	let output = biwa_cmd_tilde(&["pull"], dir.path())
		.env("BIWA_SYNC_SFTP_CACHE_PATH", cache_dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("Remote project changed while pull data was being prepared"),
		"stderr: {stderr}"
	);
	assert_eq!(fs::read_to_string(&file_path)?, "local");
	assert!(!dir.path().join("extra.txt").exists());
	Ok(())
}

#[test]
fn e2e_sync_runs_local_hooks_around_upload() -> Result<()> {
	let dir = tempfile::tempdir()?;
	common::write_hooks_config(
		dir.path(),
		Some(r#"sh -c "printf generated > generated.txt; echo pre-sync-marker""#),
		Some(r#"sh -c "printf done > post-sync.txt""#),
	)?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(output.status.success(), "stderr: {stderr}");
	// Hook output is streamed to stderr so biwa's stdout stays reserved for
	// remote command output.
	assert!(stderr.contains("pre-sync-marker"), "stderr: {stderr}");
	assert!(!stdout.contains("pre-sync-marker"), "stdout: {stdout}");
	// post_sync runs locally after the upload, so its output is never uploaded.
	assert_eq!(
		fs::read_to_string(dir.path().join("post-sync.txt"))?,
		"done"
	);

	// The file created by pre_sync is part of the same upload.
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let check = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"cat",
			&format!("{remote_proj_dir}/generated.txt"),
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		check.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&check.stderr)
	);
	assert_eq!(String::from_utf8_lossy(&check.stdout).trim(), "generated");

	let missing = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&format!("{remote_proj_dir}/post-sync.txt"),
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(missing.status.success(), "post_sync output was uploaded");

	Ok(())
}

#[test]
fn e2e_sync_hook_output_follows_quiet_and_silent() -> Result<()> {
	let dir = tempfile::tempdir()?;
	common::write_hooks_config(
		dir.path(),
		Some(r#"sh -c "echo pre-sync-marker; echo pre-sync-error >&2""#),
		None,
	)?;

	// --quiet hides hook stdout but keeps hook stderr, so a failing hook can
	// still explain itself.
	let quiet = biwa_cmd_tilde(&["--quiet", "sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&quiet.stderr);
	let stdout = String::from_utf8_lossy(&quiet.stdout);
	assert!(quiet.status.success(), "stderr: {stderr}");
	assert!(!stderr.contains("pre-sync-marker"), "stderr: {stderr}");
	assert!(stderr.contains("pre-sync-error"), "stderr: {stderr}");
	assert!(!stdout.contains("pre-sync-marker"), "stdout: {stdout}");

	// --silent hides both hook streams.
	let silent = biwa_cmd_tilde(&["--silent", "sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&silent.stderr);
	let stdout = String::from_utf8_lossy(&silent.stdout);
	assert!(silent.status.success(), "stderr: {stderr}");
	assert!(!stderr.contains("pre-sync-marker"), "stderr: {stderr}");
	assert!(!stderr.contains("pre-sync-error"), "stderr: {stderr}");
	assert!(!stdout.contains("pre-sync-marker"), "stdout: {stdout}");
	Ok(())
}

#[test]
fn e2e_sync_pre_sync_failure_aborts_before_upload() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "world")?;
	common::write_hooks_config(
		dir.path(),
		Some(r#"sh -c "exit 3""#),
		Some(r#"sh -c "printf done > post-sync.txt""#),
	)?;
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(stderr.contains("`hooks.pre_sync` hook"), "stderr: {stderr}");
	assert!(stderr.contains("exited with code 3"), "stderr: {stderr}");
	// post_sync must not run when the operation aborted.
	assert!(!dir.path().join("post-sync.txt").exists());

	let missing = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"-d",
			"~",
			"sh",
			"-c",
			"test ! -e \"$1\"",
			"sh",
			&remote_proj_dir,
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert!(
		missing.status.success(),
		"files were uploaded even though pre_sync failed"
	);
	Ok(())
}

#[test]
fn e2e_sync_post_sync_failure_fails_the_command() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("hello.txt"), "world")?;
	common::write_hooks_config(dir.path(), None, Some(r#"sh -c "exit 4""#))?;

	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("`hooks.post_sync` hook"),
		"stderr: {stderr}"
	);
	assert!(stderr.contains("exited with code 4"), "stderr: {stderr}");

	// The upload itself already completed before the hook failed.
	let remote_proj_dir = common::get_remote_project_dir(dir.path())?;
	let check = biwa_cmd_tilde(
		&[
			"run",
			"--skip-sync",
			"cat",
			&format!("{remote_proj_dir}/hello.txt"),
		],
		dir.path(),
	)
	.stdout_capture()
	.stderr_capture()
	.unchecked()
	.run()?;
	assert_eq!(String::from_utf8_lossy(&check.stdout).trim(), "world");
	Ok(())
}

#[test]
fn e2e_sync_push_failure_skips_post_sync_hook() -> Result<()> {
	let dir = tempfile::tempdir()?;
	fs::write(dir.path().join("first.txt"), "first")?;
	fs::write(dir.path().join("second.txt"), "second")?;
	common::write_hooks_config(
		dir.path(),
		Some(r#"sh -c "printf pre > pre-sync.txt""#),
		Some(r#"sh -c "printf post > post-sync.txt""#),
	)?;

	// The file limit makes the upload itself fail after the pre-sync hook ran.
	let output = biwa_cmd_tilde(&["sync"], dir.path())
		.env("BIWA_SYNC_SFTP_MAX_FILES_TO_SYNC", "1")
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success(), "stderr: {stderr}");
	assert!(
		stderr.contains("Aborting synchronization"),
		"stderr: {stderr}"
	);
	assert_eq!(fs::read_to_string(dir.path().join("pre-sync.txt"))?, "pre");
	assert!(
		!dir.path().join("post-sync.txt").exists(),
		"post_sync ran even though the upload failed"
	);
	Ok(())
}

#[test]
fn e2e_run_hooks_follow_the_sync_phase() -> Result<()> {
	let dir = tempfile::tempdir()?;
	common::write_hooks_config(
		dir.path(),
		Some(r#"sh -c "printf pre > pre-sync.txt""#),
		Some(r#"sh -c "printf post > post-sync.txt""#),
	)?;

	// Skipping the sync phase also skips both sync hooks.
	let skipped = biwa_cmd_tilde(&["run", "--skip-sync", "true"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		skipped.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&skipped.stderr)
	);
	assert!(!dir.path().join("pre-sync.txt").exists());
	assert!(!dir.path().join("post-sync.txt").exists());

	// The automatic sync phase runs both hooks.
	let synced = biwa_cmd_tilde(&["run", "true"], dir.path())
		.stdout_capture()
		.stderr_capture()
		.unchecked()
		.run()?;
	assert!(
		synced.status.success(),
		"stderr: {}",
		String::from_utf8_lossy(&synced.stderr)
	);
	assert_eq!(fs::read_to_string(dir.path().join("pre-sync.txt"))?, "pre");
	assert_eq!(
		fs::read_to_string(dir.path().join("post-sync.txt"))?,
		"post"
	);
	Ok(())
}
