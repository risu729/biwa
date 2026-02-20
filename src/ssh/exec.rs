use super::auth::resolve_auth;
use crate::config::Config;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use console::style;
use eyre::{Context, bail};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tracing::{debug, info};

/// Connect to the SSH server using the resolved authentication method.
async fn connect(config: &Config, silent: bool) -> eyre::Result<Client> {
	let auth_method = resolve_auth(config)?;
	let ssh = &config.ssh;

	let spinner = if silent {
		None
	} else {
		let sp = ProgressBar::new_spinner();
		sp.set_style(
			ProgressStyle::default_spinner()
				.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
				.template("{spinner:.cyan} {msg}")
				.expect("invalid spinner template"),
		);
		sp.set_message(format!(
			"Connecting to {}@{}:{}...",
			ssh.user, ssh.host, ssh.port
		));
		sp.enable_steady_tick(Duration::from_millis(80));
		Some(sp)
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
	let client = connect(config, silent).await?;

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

	let result = client
		.execute(&full_command)
		.await
		.wrap_err("Failed to execute remote command")?;

	if !silent {
		if !result.stdout.is_empty() {
			print!("{}", result.stdout);
		}
		if !result.stderr.is_empty() {
			eprint!("{}", result.stderr);
		}
	}

	let exit_status = result.exit_status;
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
