use crate::Result;
use crate::config::types::HooksConfig;
use color_eyre::eyre::{Context as _, bail};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use tokio::process::Command;
use tracing::info;

/// A local lifecycle hook executed around project synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncHook {
	/// Runs before local files are uploaded.
	PreSync,
	/// Runs after files were uploaded successfully.
	PostSync,
}

impl SyncHook {
	/// Returns the configuration key that defines this hook.
	const fn config_key(self) -> &'static str {
		match self {
			Self::PreSync => "hooks.pre_sync",
			Self::PostSync => "hooks.post_sync",
		}
	}

	/// Returns the command line configured for this hook, if any.
	fn command_line(self, hooks: &HooksConfig) -> Option<&str> {
		match self {
			Self::PreSync => hooks.pre_sync.as_deref(),
			Self::PostSync => hooks.post_sync.as_deref(),
		}
	}
}

/// Runs a configured synchronization hook locally in the resolved sync root.
///
/// Hooks are optional: an unset command is a no-op. A configured hook that fails
/// aborts the whole operation so a broken build never reaches the remote host.
pub(super) async fn run_sync_hook(
	hook: SyncHook,
	hooks: &HooksConfig,
	sync_root: &Path,
	quiet: bool,
) -> Result<()> {
	let Some(command_line) = hook.command_line(hooks) else {
		return Ok(());
	};
	let words = parse_hook_command(hook, command_line)?;
	let (program, arguments) = words
		.split_first()
		.expect("parse_hook_command rejects empty commands");

	info!(
		hook = hook.config_key(),
		command = command_line,
		sync_root = %sync_root.display(),
		"Running sync hook"
	);

	let status = hook_command(program, arguments, sync_root, quiet)?
		.status()
		.await
		.wrap_err_with(|| {
			format!(
				"Failed to run `{}` hook `{command_line}` in {}",
				hook.config_key(),
				sync_root.display()
			)
		})?;

	if !status.success() {
		bail!(
			"`{}` hook `{command_line}` {} (ran in {})",
			hook.config_key(),
			describe_status(status),
			sync_root.display()
		);
	}

	Ok(())
}

/// Splits a hook command line into an argument vector.
///
/// Hooks are parsed with shell word splitting instead of being handed to a shell,
/// so quoted arguments work without exposing configuration values to shell
/// expansion. Use an explicit `sh -c "..."` hook when shell features are needed.
fn parse_hook_command(hook: SyncHook, command_line: &str) -> Result<Vec<String>> {
	let argv = shell_words::split(command_line).wrap_err_with(|| {
		format!(
			"Failed to parse `{}` hook `{command_line}`",
			hook.config_key()
		)
	})?;
	if argv.is_empty() {
		bail!(
			"`{}` is empty; remove the key to disable the hook.",
			hook.config_key()
		);
	}
	Ok(argv)
}

/// Builds the local hook process with biwa's stream policy applied.
fn hook_command(
	program: &str,
	arguments: &[String],
	sync_root: &Path,
	quiet: bool,
) -> Result<Command> {
	let mut command = Command::new(program);
	command
		.args(arguments)
		.current_dir(sync_root)
		// Hooks must not consume the stdin that belongs to the remote command.
		.stdin(Stdio::null())
		// Hook stdout is redirected to biwa's stderr so that piping `biwa run`
		// output keeps yielding the remote command's stdout only.
		.stdout(hook_output(quiet)?)
		.stderr(if quiet {
			Stdio::null()
		} else {
			Stdio::inherit()
		});
	Ok(command)
}

/// Returns the stdio target for hook stdout: biwa's stderr, or nothing when quiet.
#[cfg(unix)]
fn hook_output(quiet: bool) -> Result<Stdio> {
	use std::io::stderr;
	use std::os::fd::AsFd as _;

	if quiet {
		return Ok(Stdio::null());
	}
	Ok(Stdio::from(stderr().as_fd().try_clone_to_owned()?))
}

/// Returns the stdio target for hook stdout on platforms without file descriptors.
#[cfg(not(unix))]
fn hook_output(quiet: bool) -> Result<Stdio> {
	Ok(if quiet {
		Stdio::null()
	} else {
		Stdio::inherit()
	})
}

/// Describes how a hook process terminated, including signals on Unix.
fn describe_status(status: ExitStatus) -> String {
	#[cfg(unix)]
	{
		use std::os::unix::process::ExitStatusExt as _;

		if let Some(signal) = status.signal() {
			return format!("was terminated by signal {signal}");
		}
	}
	status.code().map_or_else(
		|| "failed without an exit code".to_owned(),
		|code| format!("exited with code {code}"),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use std::fs;
	use tempfile::tempdir;

	/// Returns a hooks configuration with only `pre_sync` set.
	fn pre_sync(command: &str) -> HooksConfig {
		HooksConfig {
			pre_sync: Some(command.to_owned()),
			post_sync: None,
		}
	}

	#[test]
	fn parse_hook_command_keeps_quoted_arguments_together() -> Result<()> {
		assert_eq!(
			parse_hook_command(SyncHook::PreSync, "npm run 'build all' --flag=\"a b\"")?,
			vec!["npm", "run", "build all", "--flag=a b"]
		);
		Ok(())
	}

	#[test]
	fn parse_hook_command_rejects_empty_commands() {
		for command in ["", "   ", "\t\n"] {
			let error = parse_hook_command(SyncHook::PostSync, command).unwrap_err();
			assert!(
				error.to_string().contains("`hooks.post_sync` is empty"),
				"error was: {error}"
			);
		}
	}

	#[test]
	fn parse_hook_command_reports_parse_errors() {
		let error = parse_hook_command(SyncHook::PreSync, "echo 'unterminated").unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Failed to parse `hooks.pre_sync` hook"),
			"error was: {error}"
		);
	}

	#[tokio::test]
	async fn unset_hooks_are_skipped() -> Result<()> {
		let dir = tempdir()?;
		let hooks = HooksConfig {
			pre_sync: None,
			post_sync: None,
		};

		run_sync_hook(SyncHook::PreSync, &hooks, dir.path(), true).await?;
		run_sync_hook(SyncHook::PostSync, &hooks, dir.path(), true).await?;
		Ok(())
	}

	#[tokio::test]
	async fn hooks_run_in_the_sync_root() -> Result<()> {
		let dir = tempdir()?;
		let sync_root = dir.path().join("project");
		fs::create_dir_all(&sync_root)?;

		run_sync_hook(
			SyncHook::PreSync,
			&pre_sync("sh -c 'printf generated > generated.txt'"),
			&sync_root,
			true,
		)
		.await?;

		assert_eq!(
			fs::read_to_string(sync_root.join("generated.txt"))?,
			"generated"
		);
		Ok(())
	}

	#[tokio::test]
	async fn failing_hooks_report_the_command_and_exit_code() -> Result<()> {
		let dir = tempdir()?;

		let error = run_sync_hook(
			SyncHook::PostSync,
			&HooksConfig {
				pre_sync: None,
				post_sync: Some("sh -c 'exit 3'".to_owned()),
			},
			dir.path(),
			true,
		)
		.await
		.unwrap_err();

		let message = error.to_string();
		assert!(message.contains("`hooks.post_sync` hook"), "{message}");
		assert!(message.contains("sh -c 'exit 3'"), "{message}");
		assert!(message.contains("exited with code 3"), "{message}");
		Ok(())
	}

	#[tokio::test]
	async fn missing_hook_programs_report_the_command() -> Result<()> {
		let dir = tempdir()?;

		let error = run_sync_hook(
			SyncHook::PreSync,
			&pre_sync("biwa-hook-command-that-does-not-exist"),
			dir.path(),
			true,
		)
		.await
		.unwrap_err();

		let message = format!("{error:#}");
		assert!(
			message.contains("Failed to run `hooks.pre_sync` hook"),
			"{message}"
		);
		assert!(
			message.contains("biwa-hook-command-that-does-not-exist"),
			"{message}"
		);
		Ok(())
	}
}
