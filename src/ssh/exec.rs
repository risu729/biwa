use super::auth::resolve_auth;
use crate::Result;
use crate::config::types::Config;
use crate::ui::create_spinner;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use color_eyre::eyre::{Context as _, bail};
use console::style;
use std::io::{Write, stderr, stdout};
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Connect to the SSH server using the resolved authentication method.
async fn connect(config: &Config, quiet: bool) -> Result<Client> {
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

	let client = Client::connect(
		(ssh.host.as_str(), ssh.port),
		ssh.user.as_str(),
		auth_method,
		ServerCheckMethod::NoCheck,
	)
	.await
	.wrap_err_with(|| {
		format!(
			"Failed to connect to {}@{}:{}",
			ssh.user, ssh.host, ssh.port
		)
	})?;

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
		parts.extend(args.iter().map(|a| shell_words::quote(a).into_owned()));
		parts.join(" ")
	}
}

/// Write bytes to a locked standard stream, ignoring errors.
fn write_to_stream(mut stream: impl Write, bytes: &[u8]) {
	if let Err(e) = stream.write_all(bytes) {
		debug!(error = %e, "Failed to write to stream");
	}
	if let Err(e) = stream.flush() {
		debug!(error = %e, "Failed to flush stream");
	}
}

/// Run a pre-built command string on an already-connected SSH client.
///
/// Returns the remote exit code, printing stdout/stderr as they arrive
/// unless `silent` is set.
#[expect(
	clippy::integer_division_remainder_used,
	reason = "tokio::select! macro expands to use % internally"
)]
async fn run_command(
	client: &Client,
	full_command: &str,
	quiet: bool,
	silent: bool,
) -> Result<u32> {
	debug!(command = %full_command, "Executing remote command");

	if !quiet {
		eprintln!(
			"{} {}",
			style("$").cyan().bold(),
			style(full_command).bold()
		);
	}
	let (stdout_tx, mut stdout_rx) = mpsc::channel(1024);
	let (stderr_tx, mut stderr_rx) = mpsc::channel(1024);

	let exec_future =
		client.execute_io(full_command, stdout_tx, Some(stderr_tx), None, false, None);
	tokio::pin!(exec_future);

	let exit_status = loop {
		tokio::select! {
			res = &mut exec_future => {
				break res.wrap_err("Failed to execute remote command")?;
			},
			Some(stdout_bytes) = stdout_rx.recv() => {
				if !silent {
					write_to_stream(stdout().lock(), &stdout_bytes);
				}
			},
			Some(stderr_bytes) = stderr_rx.recv() => {
				if !silent {
					write_to_stream(stderr().lock(), &stderr_bytes);
				}
			},
		}
	};

	if !silent {
		while let Some(stdout_bytes) = stdout_rx.recv().await {
			write_to_stream(stdout().lock(), &stdout_bytes);
		}
		while let Some(stderr_bytes) = stderr_rx.recv().await {
			write_to_stream(stderr().lock(), &stderr_bytes);
		}
	}
	debug!(exit_status, "Remote command completed");

	if exit_status != 0 && !quiet {
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
/// Returns the exit code of the remote command.
pub async fn execute_command(
	config: &Config,
	command: &str,
	args: &[String],
	quiet: bool,
	silent: bool,
) -> Result<u32> {
	let client = connect(config, quiet || silent).await?;
	let full_command = build_command(command, args);
	run_command(&client, &full_command, quiet, silent).await
}

#[cfg(test)]
mod tests {
	use super::*;

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
