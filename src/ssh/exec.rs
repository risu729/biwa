use super::auth::resolve_auth;
use crate::config::SshConfig;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use console::style;
use eyre::{Context, bail};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tracing::{debug, info};

/// Connect to the SSH server using the resolved authentication method.
async fn connect(config: &SshConfig, silent: bool) -> eyre::Result<Client> {
	let auth_method = resolve_auth(config)?;

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
			config.user, config.host, config.port
		));
		sp.enable_steady_tick(Duration::from_millis(80));
		Some(sp)
	};

	let client = Client::connect(
		(config.host.as_str(), config.port),
		config.user.as_str(),
		auth_method,
		ServerCheckMethod::NoCheck,
	)
	.await
	.wrap_err_with(|| {
		format!(
			"Failed to connect to {}@{}:{}",
			config.user, config.host, config.port
		)
	})?;

	spinner.as_ref().inspect(|s| s.finish_and_clear());
	info!(
		host = %config.host,
		port = config.port,
		user = %config.user,
		"Connected to SSH server"
	);

	Ok(client)
}

/// Execute a command on the remote host via SSH.
///
/// Returns the exit code of the remote command.
pub async fn execute_command(
	config: &SshConfig,
	command: &str,
	args: &[String],
	quiet: bool,
	silent: bool,
) -> eyre::Result<u32> {
	let client = connect(config, silent).await?;

	let full_command = if args.is_empty() {
		command.to_string()
	} else {
		format!(
			"{} {}",
			command,
			args.iter()
				.map(|a| shell_escape(a))
				.collect::<Vec<_>>()
				.join(" ")
		)
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

/// Escape a shell argument if it contains special characters.
fn shell_escape(arg: &str) -> String {
	if arg.is_empty() {
		return "''".to_string();
	}

	if arg
		.chars()
		.all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '=' | '@'))
	{
		return arg.to_string();
	}

	format!("'{}'", arg.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_shell_escape_simple() {
		assert_eq!(shell_escape("hello"), "hello");
		assert_eq!(shell_escape("hello-world"), "hello-world");
		assert_eq!(shell_escape("/path/to/file"), "/path/to/file");
	}

	#[test]
	fn test_shell_escape_special_chars() {
		assert_eq!(shell_escape("hello world"), "'hello world'");
		assert_eq!(shell_escape("it's"), "'it'\\''s'");
		assert_eq!(shell_escape(""), "''");
	}

	#[test]
	fn test_shell_escape_safe_chars() {
		assert_eq!(shell_escape("user@host:22"), "user@host:22");
		assert_eq!(shell_escape("key=value"), "key=value");
		assert_eq!(shell_escape("file.txt"), "file.txt");
	}
}
