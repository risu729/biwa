use super::auth::resolve_auth;
use crate::config::Config;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use console::style;
use eyre::{Context, bail};
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

	let full_command = if args.is_empty() {
		command.to_string()
	} else {
		let mut parts = vec![command.to_string()];
		parts.extend(args.iter().map(|a| shell_words::quote(a).into_owned()));
		parts.join(" ")
	};

	debug!(command = %full_command, "Executing remote command");

	if !quiet {
		eprintln!(
			"{} {}",
			style("$").cyan().bold(),
			style(&full_command).bold()
		);
	}

	let (stdout_tx, mut stdout_rx) = tokio::sync::mpsc::channel(1024);
	let (stderr_tx, mut stderr_rx) = tokio::sync::mpsc::channel(1024);

	let exec_future =
		client.execute_io(&full_command, stdout_tx, Some(stderr_tx), None, false, None);
	tokio::pin!(exec_future);

	let exit_status = loop {
		tokio::select! {
			res = &mut exec_future => {
				break res.wrap_err("Failed to execute remote command")?;
			},
			Some(stdout) = stdout_rx.recv() => {
				if !silent {
					use std::io::Write;
					let mut lock = std::io::stdout().lock();
					let _ = lock.write_all(&stdout);
					let _ = lock.flush();
				}
			},
			Some(stderr) = stderr_rx.recv() => {
				if !silent {
					use std::io::Write;
					let mut lock = std::io::stderr().lock();
					let _ = lock.write_all(&stderr);
					let _ = lock.flush();
				}
			},
		}
	};

	if !silent {
		use std::io::Write;
		while let Some(stdout) = stdout_rx.recv().await {
			let mut lock = std::io::stdout().lock();
			let _ = lock.write_all(&stdout);
			let _ = lock.flush();
		}
		while let Some(stderr) = stderr_rx.recv().await {
			let mut lock = std::io::stderr().lock();
			let _ = lock.write_all(&stderr);
			let _ = lock.flush();
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
