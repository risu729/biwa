use super::auth::resolve_auth;
use crate::config::types::Config;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use console::style;
use eyre::{Context as _, bail};
use tracing::{debug, info};

/// Connect to the SSH server using the resolved authentication method.
async fn connect(config: &Config, silent: bool) -> eyre::Result<Client> {
	let auth_method = resolve_auth(config)?;
	let ssh = &config.ssh;

	let spinner = if silent {
		None
	} else {
		Some(crate::ui::create_spinner(format!(
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
pub fn build_command(command: &str, args: &[String]) -> String {
	if args.is_empty() {
		command.to_owned()
	} else {
		let mut parts = vec![command.to_owned()];
		parts.extend(args.iter().map(|a| shell_words::quote(a).into_owned()));
		parts.join(" ")
	}
}

/// Write bytes to a locked standard stream, ignoring errors.
fn write_to_stream(mut stream: impl std::io::Write, bytes: &[u8]) {
	let _ = stream.write_all(bytes);
	let _ = stream.flush();
}

/// Run a pre-built command string on an already-connected SSH client.
///
/// Returns the remote exit code, printing stdout/stderr as they arrive
/// unless `silent` is set.
pub async fn run_command(
	client: &Client,
	full_command: &str,
	quiet: bool,
	silent: bool,
) -> eyre::Result<u32> {
	debug!(command = %full_command, "Executing remote command");

	if !quiet {
		eprintln!(
			"{} {}",
			style("$").cyan().bold(),
			style(full_command).bold()
		);
	}

	let (stdout_tx, mut stdout_rx) = tokio::sync::mpsc::channel(1024);
	let (stderr_tx, mut stderr_rx) = tokio::sync::mpsc::channel(1024);

	let exec_future =
		client.execute_io(full_command, stdout_tx, Some(stderr_tx), None, false, None);
	tokio::pin!(exec_future);

	let exit_status = loop {
		tokio::select! {
			res = &mut exec_future => {
				break res.wrap_err("Failed to execute remote command")?;
			},
			Some(stdout) = stdout_rx.recv() => {
				if !silent {
					write_to_stream(std::io::stdout().lock(), &stdout);
				}
			},
			Some(stderr) = stderr_rx.recv() => {
				if !silent {
					write_to_stream(std::io::stderr().lock(), &stderr);
				}
			},
		}
	};

	if !silent {
		while let Some(stdout) = stdout_rx.recv().await {
			write_to_stream(std::io::stdout().lock(), &stdout);
		}
		while let Some(stderr) = stderr_rx.recv().await {
			write_to_stream(std::io::stderr().lock(), &stderr);
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
) -> eyre::Result<u32> {
	let client = connect(config, quiet || silent).await?;
	let full_command = build_command(command, args);
	run_command(&client, &full_command, quiet, silent).await
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_build_command_no_args() {
		assert_eq!(build_command("ls", &[]), "ls");
	}

	#[test]
	fn test_build_command_with_args() {
		let args = vec!["-la".to_string(), "/tmp".to_string()];
		assert_eq!(build_command("ls", &args), "ls -la /tmp");
	}

	#[test]
	fn test_build_command_quotes_args_with_spaces() {
		let args = vec!["hello world".to_string()];
		assert_eq!(build_command("echo", &args), "echo 'hello world'");
	}

	#[test]
	fn test_build_command_quotes_args_with_special_chars() {
		let args = vec!["foo$bar".to_string()];
		assert_eq!(build_command("echo", &args), "echo 'foo$bar'");
	}
}
