use super::auth::resolve_auth;
use super::sync::shell_quote_path;
use crate::Result;
use crate::config::types::Config;
use crate::ui::create_spinner;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use bytes::Bytes;
use color_eyre::eyre::{Context as _, bail};
use console::style;
use core::time::Duration;
use std::io::Error as IoError;
use tokio::io::{copy, stderr, stdout};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

/// Connect to the SSH server using the resolved authentication method.
pub(super) async fn connect(config: &Config, quiet: bool) -> Result<Client> {
	let auth_method = resolve_auth(config)?;
	let ssh = &config.ssh;

	let spinner = if quiet {
		None
	} else {
		Some(create_spinner(format!(
			"Connecting to {}@{}:{}...",
			ssh.user, ssh.host, ssh.port
		)))
	};

	let mut retries = 3_usize;
	let mut delay = Duration::from_millis(500);

	let client = loop {
		match Client::connect(
			(ssh.host.as_str(), ssh.port),
			ssh.user.as_str(),
			auth_method.clone(),
			ServerCheckMethod::NoCheck,
		)
		.await
		{
			Ok(c) => break c,
			Err(e) if retries > 0 => {
				debug!(
					error = %e,
					retry_delay_ms = delay.as_millis(),
					retries_remaining = retries,
					"Failed to connect to SSH server; retrying"
				);
				sleep(delay).await;
				retries = retries.saturating_sub(1);
				delay = delay.saturating_mul(2);
			}
			Err(e) => {
				return Err(e).wrap_err_with(|| {
					format!(
						"Failed to connect to {}@{}:{}",
						ssh.user, ssh.host, ssh.port
					)
				});
			}
		}
	};

	spinner.as_ref().inspect(|s| s.finish_and_clear());
	info!(
		host = %ssh.host,
		port = ssh.port,
		user = %ssh.user,
		"Connected to SSH server"
	);

	Ok(client)
}

/// Build the full shell command string from a command and its arguments.
///
/// Arguments are shell-quoted so they round-trip safely.
fn build_command(command: &str, args: &[String]) -> String {
	if args.is_empty() {
		command.to_owned()
	} else {
		let mut parts = vec![command.to_owned()];
		parts.extend(args.iter().map(|a| shell_quote_path(a)));
		parts.join(" ")
	}
}

/// Run a pre-built command string on an already-connected SSH client.
///
/// Returns the remote exit code, printing stdout/stderr as they arrive
/// unless `silent` is set.
///
/// If `working_dir` is set, the command is executed after `cd`-ing into that
/// directory. If the directory does not exist, it will be created first.
async fn run_command(
	client: &Client,
	full_command: &str,
	working_dir: Option<&str>,
	umask: &str,
	quiet: bool,
	silent: bool,
) -> Result<u32> {
	u32::from_str_radix(umask, 8).wrap_err_with(|| format!("Invalid umask: {umask}"))?;
	let effective_command = working_dir.map_or_else(
		|| format!("umask {umask} && {full_command}"),
		|dir| {
			let quoted_dir = shell_quote_path(dir);
			format!(
				"umask {umask} && mkdir -p -- {quoted_dir} && cd {quoted_dir} && {full_command}"
			)
		},
	);
	debug!(command = %effective_command, "Executing remote command");

	if !quiet {
		eprintln!(
			"{} {}",
			style("$").cyan().bold(),
			style(full_command).bold()
		);
	}
	let (stdout_tx, stdout_rx) = mpsc::channel(1024);
	let (stderr_tx, stderr_rx) = mpsc::channel(1024);

	let stdout_stream = ReceiverStream::new(stdout_rx).map(|b| Ok::<_, IoError>(Bytes::from(b)));
	let stderr_stream = ReceiverStream::new(stderr_rx).map(|b| Ok::<_, IoError>(Bytes::from(b)));

	let mut stdout_reader = StreamReader::new(stdout_stream);
	let mut stderr_reader = StreamReader::new(stderr_stream);

	let exec_future = client.execute_io(
		&effective_command,
		stdout_tx,
		Some(stderr_tx),
		None,
		false,
		None,
	);

	let stdout_task = async {
		if !silent {
			copy(&mut stdout_reader, &mut stdout()).await.unwrap_or(0);
		}
	};

	let stderr_task = async {
		if !silent {
			copy(&mut stderr_reader, &mut stderr()).await.unwrap_or(0);
		}
	};

	let (exit_status, (), ()) = tokio::join!(exec_future, stdout_task, stderr_task);
	let exit_status = exit_status.wrap_err("Failed to execute remote command")?;

	debug!(exit_status, "Remote command completed");

	if exit_status != 0 && !quiet {
		warn!(exit_status, "Remote command exited with non-zero status");
		eprintln!(
			"{} Process exited with code {}",
			style("✗").red().bold(),
			style(exit_status).red()
		);
	}

	if exit_status != 0 {
		bail!("Remote command exited with code {exit_status}");
	}

	Ok(exit_status)
}

/// Execute a command on the remote host via SSH.
///
/// If `working_dir` is set, the command executes inside that remote directory.
/// Returns the exit code of the remote command.
pub async fn execute_command(
	config: &Config,
	command: &str,
	args: &[String],
	working_dir: Option<&str>,
	quiet: bool,
	silent: bool,
) -> Result<u32> {
	info!(
		command,
		args_count = args.len(),
		has_working_dir = working_dir.is_some(),
		quiet,
		silent,
		"Starting remote command execution"
	);
	let client = connect(config, quiet || silent).await?;
	let full_command = build_command(command, args);
	run_command(
		&client,
		&full_command,
		working_dir,
		&config.ssh.umask,
		quiet,
		silent,
	)
	.await
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn build_command_no_args() {
		assert_eq!(build_command("ls", &[]), "ls");
	}

	#[test]
	fn build_command_with_args() {
		let args = vec!["-la".to_owned(), "/tmp".to_owned()];
		assert_eq!(build_command("ls", &args), "ls -la /tmp");
	}

	#[test]
	fn build_command_quotes_args_with_spaces() {
		let args = vec!["hello world".to_owned()];
		assert_eq!(build_command("echo", &args), "echo 'hello world'");
	}

	#[test]
	fn build_command_quotes_args_with_special_chars() {
		let args = vec!["foo$bar".to_owned()];
		assert_eq!(build_command("echo", &args), "echo 'foo$bar'");
	}
}
