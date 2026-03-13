use super::auth::resolve_auth;
use super::sync::shell_quote_remote_value;
use crate::Result;
use crate::config::types::Config;
use crate::env_vars::{
	EnvForwardMethod, EnvVarSource, EnvVarSpec, is_environment_dependent_env_var, merge_env_vars,
};
use crate::ui::create_spinner;
use async_ssh2_tokio::client::{Client, ServerCheckMethod};
use bytes::Bytes;
use color_eyre::eyre::{Context as _, bail};
use console::style;
use core::time::Duration;
use russh::{Channel, ChannelMsg, client::Msg};
use std::env;
use std::io::Error as IoError;
use tokio::io::{copy, stderr, stdout};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

/// Resolved environment variable to send remotely.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedEnvVar {
	/// Environment variable name.
	name: String,
	/// Concrete value to send to the remote process.
	value: String,
}

/// Static execution settings reused across remote command helpers.
struct RunCommandOptions<'a> {
	/// Remote working directory to enter before running the command.
	working_dir: Option<&'a str>,
	/// Remote umask to apply before command execution.
	umask: &'a str,
	/// Suppresses local progress output when true.
	quiet: bool,
	/// Suppresses forwarded remote stdout/stderr when true.
	silent: bool,
}

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
		parts.extend(args.iter().map(|a| shell_quote_remote_value(a)));
		parts.join(" ")
	}
}

/// Builds shell export statements for environment variables.
fn build_export_prefix(env_vars: &[ResolvedEnvVar]) -> String {
	if env_vars.is_empty() {
		String::new()
	} else {
		let exports = env_vars.iter().map(|env_var| {
			format!(
				"export {}={}",
				env_var.name,
				shell_quote_remote_value(&env_var.value)
			)
		});
		format!("{} && ", exports.collect::<Vec<_>>().join(" && "))
	}
}

/// Resolves config and CLI environment variable settings into concrete values.
fn resolve_env_vars(config: &Config, cli_env_vars: &[EnvVarSpec]) -> Result<Vec<ResolvedEnvVar>> {
	let mut specs = config.env.vars.specs()?;
	specs.extend_from_slice(cli_env_vars);

	merge_env_vars(specs)
		.into_iter()
		.map(|spec| {
			if spec.is_inherited() && is_environment_dependent_env_var(spec.name()) {
				warn!(
					env_var = spec.name(),
					"Inheriting an environment-dependent variable from the local machine"
				);
			}

			let value = match spec.source() {
				EnvVarSource::Inherit => env::var(spec.name()).wrap_err_with(|| {
					format!("Environment variable `{}` is not set locally", spec.name())
				})?,
				EnvVarSource::Value(value) => value.clone(),
			};

			Ok(ResolvedEnvVar {
				name: spec.name().to_owned(),
				value,
			})
		})
		.collect()
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
	env_vars: &[ResolvedEnvVar],
	forward_method: &EnvForwardMethod,
	options: RunCommandOptions<'_>,
) -> Result<u32> {
	u32::from_str_radix(options.umask, 8)
		.wrap_err_with(|| format!("Invalid umask: {}", options.umask))?;
	let command_with_env = match forward_method {
		EnvForwardMethod::Export => format!("{}{}", build_export_prefix(env_vars), full_command),
		EnvForwardMethod::Setenv => full_command.to_owned(),
	};
	let effective_command = options.working_dir.map_or_else(
		|| format!("umask {} && {command_with_env}", options.umask),
		|dir| {
			let quoted_dir = shell_quote_remote_value(dir);
			format!(
				"umask {} && mkdir -p -- {quoted_dir} && cd {quoted_dir} && {command_with_env}",
				options.umask,
			)
		},
	);
	debug!(command = %effective_command, "Executing remote command");

	if !options.quiet {
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

	let exec_future = execute_with_forward_method(
		client,
		&effective_command,
		env_vars,
		forward_method,
		stdout_tx,
		stderr_tx,
	);

	let stdout_task = async {
		if !options.silent {
			copy(&mut stdout_reader, &mut stdout()).await.unwrap_or(0);
		}
	};

	let stderr_task = async {
		if !options.silent {
			copy(&mut stderr_reader, &mut stderr()).await.unwrap_or(0);
		}
	};

	let (exit_status, (), ()) = tokio::join!(exec_future, stdout_task, stderr_task);
	let exit_status = exit_status.wrap_err("Failed to execute remote command")?;

	debug!(exit_status, "Remote command completed");

	if exit_status != 0 && !options.quiet {
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
	cli_env_vars: &[EnvVarSpec],
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
	let env_vars = resolve_env_vars(config, cli_env_vars)?;
	run_command(
		&client,
		&full_command,
		&env_vars,
		&config.env.forward_method,
		RunCommandOptions {
			working_dir,
			umask: &config.ssh.umask,
			quiet,
			silent,
		},
	)
	.await
}

/// Executes the remote command using either shell exports or SSH `setenv`.
async fn execute_with_forward_method(
	client: &Client,
	command: &str,
	env_vars: &[ResolvedEnvVar],
	forward_method: &EnvForwardMethod,
	stdout_tx: mpsc::Sender<Vec<u8>>,
	stderr_tx: mpsc::Sender<Vec<u8>>,
) -> Result<u32> {
	match forward_method {
		EnvForwardMethod::Export => client
			.execute_io(command, stdout_tx, Some(stderr_tx), None, false, None)
			.await
			.wrap_err("Failed to execute remote command"),
		EnvForwardMethod::Setenv => {
			let mut channel = client
				.get_channel()
				.await
				.wrap_err("Failed to open SSH session channel")?;

			for env_var in env_vars {
				channel
					.set_env(true, &env_var.name, &env_var.value)
					.await
					.wrap_err_with(|| {
						format!("Failed to send environment variable `{}`", env_var.name)
					})?;

				match channel.wait().await {
					Some(ChannelMsg::Success) => {}
					Some(ChannelMsg::Failure) => {
						warn!(
							env_var = env_var.name,
							"SSH server rejected setenv request; UNSW CSE does not support setenv, so use env.forward_method = \"export\" there"
						);
						bail!("SSH server rejected environment variable forwarding via setenv")
					}
					Some(message) => {
						bail!("Unexpected SSH response after setenv: {message:?}")
					}
					None => bail!("SSH channel closed while sending environment variables"),
				}
			}

			channel
				.exec(true, command)
				.await
				.wrap_err("Failed to execute remote command")?;

			stream_channel_output(channel, stdout_tx, stderr_tx).await
		}
	}
}

/// Streams SSH channel stdout/stderr into local output buffers until exit.
async fn stream_channel_output(
	mut channel: Channel<Msg>,
	stdout_tx: mpsc::Sender<Vec<u8>>,
	stderr_tx: mpsc::Sender<Vec<u8>>,
) -> Result<u32> {
	let mut exit_status = None;

	loop {
		match channel.wait().await {
			Some(ChannelMsg::Data { data }) => {
				stdout_tx
					.send(data.to_vec())
					.await
					.wrap_err("Failed to forward remote stdout")?;
			}
			Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
				stderr_tx
					.send(data.to_vec())
					.await
					.wrap_err("Failed to forward remote stderr")?;
			}
			Some(ChannelMsg::ExitStatus {
				exit_status: status,
			}) => exit_status = Some(status),
			Some(_) => {}
			None => break,
		}
	}

	exit_status
		.ok_or_else(|| color_eyre::eyre::eyre!("Remote command did not report an exit status"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::types::EnvConfig;
	use crate::env_vars::{EnvForwardMethod, EnvVars};
	use crate::testing::EnvCleanup;
	use pretty_assertions::assert_eq;
	use serial_test::serial;

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

	#[test]
	fn build_export_prefix_quotes_values() {
		assert_eq!(
			build_export_prefix(&[ResolvedEnvVar {
				name: "API_KEY".to_owned(),
				value: "hello world".to_owned(),
			}]),
			"export API_KEY='hello world' && "
		);
	}

	#[serial]
	#[test]
	fn resolve_env_vars_merges_config_and_cli_values() -> Result<()> {
		let config = Config {
			env: EnvConfig {
				vars: EnvVars::from_specs(vec![EnvVarSpec::inherit("NODE_ENV")]),
				forward_method: EnvForwardMethod::Export,
			},
			..Config::default()
		};

		// SAFETY: This test only mutates the current process environment.
		unsafe {
			env::set_var("NODE_ENV", "development");
		}
		let _cleanup = EnvCleanup("NODE_ENV");
		let resolved = resolve_env_vars(&config, &[EnvVarSpec::value("NODE_ENV", "production")])?;
		assert_eq!(
			resolved,
			vec![ResolvedEnvVar {
				name: "NODE_ENV".to_owned(),
				value: "production".to_owned(),
			}]
		);
		Ok(())
	}
}
