use super::auth::resolve_auth;
use super::sync::shell_quote_path;
use crate::Result;
use crate::config::types::{Config, Umask};
use crate::env_vars::{
	EnvForwardMethod, EnvVarRule, EnvVarSource, is_environment_dependent_env_var,
	local_env_var_names, resolve_env_var_rules,
};
use crate::ssh::client::Client;
use crate::ssh::client::auth::AuthenticationFailed;
use crate::ssh::client::execute::await_channel_confirmation;
use crate::ui::create_spinner;
use bytes::Bytes;
use color_eyre::eyre::{Context as _, Report, bail};
use console::style;
use core::time::Duration;
use indicatif::ProgressBar;
use russh::{Channel, ChannelMsg, Pty, client::Msg};
use std::env;
use std::io::{Error as IoError, IsTerminal as _, Read as _, stdin as std_stdin};
use std::thread;
use tokio::io::{copy, sink, stderr, stdout};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

/// Clears the spinner on drop so early returns and errors do not leave a stuck spinner.
struct SpinnerGuard(Option<ProgressBar>);

impl Drop for SpinnerGuard {
	fn drop(&mut self) {
		if let Some(s) = self.0.take() {
			s.finish_and_clear();
		}
	}
}

/// Returns true when `report` includes the structured authentication failure marker (including
/// context added via `wrap_err`).
fn report_is_authentication_failure(report: &Report) -> bool {
	report.downcast_ref::<AuthenticationFailed>().is_some()
}

/// Resolved environment variable to send remotely.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedEnvVar {
	/// Environment variable name.
	name: String,
	/// Concrete value to send to the remote process.
	value: String,
}

/// Receiver carrying stdin chunks or an EOF marker for a remote command.
type StdinReceiver = mpsc::Receiver<Option<Vec<u8>>>;

/// Static execution settings reused across remote command helpers.
struct RunCommandOptions<'a> {
	/// Remote working directory to enter before running the command.
	working_dir: Option<&'a str>,
	/// Remote umask to apply before command execution.
	umask: &'a Umask,
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

	let _spinner_cleanup = SpinnerGuard(spinner);

	let mut retries = 3_usize;
	let mut delay = Duration::from_millis(500);

	let client = loop {
		match Client::connect(
			(ssh.host.as_str(), ssh.port),
			ssh.user.as_str(),
			auth_method.clone(),
		)
		.await
		{
			Ok(c) => break c,
			Err(e) if retries > 0 => {
				if report_is_authentication_failure(&e) {
					return Err(e).wrap_err_with(|| {
						format!("Failed to authenticate as {}@{}", ssh.user, ssh.host)
					});
				}
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

/// Builds shell export statements for environment variables.
fn build_export_prefix(env_vars: &[ResolvedEnvVar]) -> String {
	if env_vars.is_empty() {
		String::new()
	} else {
		let exports = env_vars.iter().map(|env_var| {
			format!(
				"export {}={}",
				env_var.name,
				shell_words::quote(&env_var.value)
			)
		});
		format!("{} && ", exports.collect::<Vec<_>>().join(" && "))
	}
}

/// Resolves config and CLI environment variable settings into concrete values.
fn resolve_env_vars(config: &Config, cli_env_vars: &[EnvVarRule]) -> Result<Vec<ResolvedEnvVar>> {
	let mut rules = config.env.vars.rules()?;
	rules.extend_from_slice(cli_env_vars);
	let specs = resolve_env_var_rules(rules, &local_env_var_names());

	specs
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

/// Spawns a task that forwards local stdin into a channel for the remote SSH command.
fn spawn_stdin_forwarder() -> StdinReceiver {
	let (stdin_tx, stdin_rx) = mpsc::channel(32);

	thread::spawn(move || {
		let mut local_stdin = std_stdin();
		let mut buffer = vec![0_u8; 8 * 1024];

		loop {
			match local_stdin.read(&mut buffer) {
				Ok(bytes_read) if bytes_read > 0 => {
					let chunk = buffer
						.get(..bytes_read)
						.expect("stdin read length must not exceed the buffer length")
						.to_vec();

					if stdin_tx.blocking_send(Some(chunk)).is_err() {
						break;
					}
				}
				result => {
					if let Err(error) = result {
						debug!(%error, "Failed to read local stdin for remote command");
					}

					drop(stdin_tx.blocking_send(None));
					break;
				}
			}
		}
	});

	stdin_rx
}

/// Initializes stdin forwarding for the remote command.
fn prepare_stdin_forwarding() -> StdinReceiver {
	spawn_stdin_forwarder()
}

/// Returns whether local stdin is an interactive terminal.
fn stdin_is_terminal() -> bool {
	std_stdin().is_terminal()
}

/// Returns the terminal type to advertise to the SSH server.
fn local_terminal_type() -> String {
	env::var("TERM")
		.ok()
		.filter(|term| !term.trim().is_empty())
		.unwrap_or_else(|| "xterm".to_owned())
}

/// Reads a positive terminal dimension from the environment or falls back to a default.
fn terminal_dimension(var_name: &str, default: u32) -> u32 {
	env::var(var_name)
		.ok()
		.and_then(|value| value.parse::<u32>().ok())
		.filter(|value| *value > 0)
		.unwrap_or(default)
}

/// Requests an interactive PTY for terminal-backed stdin so commands can complete without EOF.
async fn request_terminal_pty(channel: &mut Channel<Msg>) -> Result<()> {
	channel
		.request_pty(
			true,
			&local_terminal_type(),
			terminal_dimension("COLUMNS", 80),
			terminal_dimension("LINES", 24),
			0,
			0,
			&[(Pty::ECHO, 0)],
		)
		.await
		.wrap_err("Failed to request SSH PTY")?;
	await_channel_confirmation(channel, "SSH PTY request").await
}

/// I/O streams and stdin mode shared by both SSH environment forwarding paths.
struct ExecuteCommandStreams {
	/// Buffered remote stdout sink.
	stdout_tx: mpsc::Sender<Vec<u8>>,
	/// Buffered remote stderr sink.
	stderr_tx: mpsc::Sender<Vec<u8>>,
	/// Local stdin receiver, or `None` once EOF has been forwarded.
	stdin_rx: Option<StdinReceiver>,
	/// Whether local stdin is attached to a terminal.
	stdin_is_terminal: bool,
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
	let command_with_env = match forward_method {
		EnvForwardMethod::Export => format!("{}{}", build_export_prefix(env_vars), full_command),
		EnvForwardMethod::Setenv => full_command.to_owned(),
	};
	let effective_command = options.working_dir.map_or_else(
		|| format!("umask {} && {command_with_env}", options.umask),
		|dir| {
			let quoted_dir = shell_quote_path(dir);
			format!(
				"umask {} && mkdir -p -- {quoted_dir} && cd {quoted_dir} && {command_with_env}",
				options.umask,
			)
		},
	);
	if tracing::enabled!(tracing::Level::DEBUG) {
		let env_var_names: Vec<&str> = env_vars
			.iter()
			.map(|env_var| env_var.name.as_str())
			.collect();

		match forward_method {
			EnvForwardMethod::Export => debug!(
				command = %full_command,
				forward_method = ?forward_method,
				working_dir = options.working_dir,
				umask = %options.umask,
				env_var_names = ?env_var_names,
				"Executing remote command"
			),
			EnvForwardMethod::Setenv => debug!(
				command = %full_command,
				effective_command = %effective_command,
				forward_method = ?forward_method,
				working_dir = options.working_dir,
				umask = %options.umask,
				env_var_names = ?env_var_names,
				"Executing remote command"
			),
		}
	}

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
	let stdin_is_terminal = stdin_is_terminal();
	let stdin_rx = Some(prepare_stdin_forwarding());

	let mut stdout_reader = StreamReader::new(stdout_stream);
	let mut stderr_reader = StreamReader::new(stderr_stream);

	let exec_future = execute_with_forward_method(
		client,
		&effective_command,
		env_vars,
		forward_method,
		ExecuteCommandStreams {
			stdout_tx,
			stderr_tx,
			stdin_rx,
			stdin_is_terminal,
		},
	);

	let stdout_task = async {
		if options.silent {
			copy(&mut stdout_reader, &mut sink()).await.unwrap_or(0);
		} else {
			copy(&mut stdout_reader, &mut stdout()).await.unwrap_or(0);
		}
	};

	let stderr_task = async {
		if options.silent {
			copy(&mut stderr_reader, &mut sink()).await.unwrap_or(0);
		} else {
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
	cli_env_vars: &[EnvVarRule],
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
	streams: ExecuteCommandStreams,
) -> Result<u32> {
	let ExecuteCommandStreams {
		stdout_tx,
		stderr_tx,
		stdin_rx,
		stdin_is_terminal,
	} = streams;

	match forward_method {
		EnvForwardMethod::Export => {
			let mut channel = client
				.get_channel()
				.await
				.wrap_err("Failed to open SSH session channel")?;

			if stdin_is_terminal {
				request_terminal_pty(&mut channel).await?;
			}

			channel
				.exec(true, command)
				.await
				.wrap_err("Failed to execute remote command")?;
			await_channel_confirmation(&mut channel, "remote command exec request").await?;

			stream_channel_output(channel, stdout_tx, stderr_tx, stdin_rx).await
		}
		EnvForwardMethod::Setenv => {
			let mut channel = client
				.get_channel()
				.await
				.wrap_err("Failed to open SSH session channel")?;

			if stdin_is_terminal {
				request_terminal_pty(&mut channel).await?;
			}

			for env_var in env_vars {
				channel
					.set_env(true, &env_var.name, &env_var.value)
					.await
					.wrap_err_with(|| {
						format!("Failed to send environment variable `{}`", env_var.name)
					})?;

				loop {
					match channel.wait().await {
						Some(ChannelMsg::Success) => {
							break;
						}
						Some(ChannelMsg::Failure) => {
							warn!(
								env_var = env_var.name,
								"SSH server rejected setenv request; UNSW CSE does not support setenv, so use env.forward_method = \"export\" there"
							);
							bail!("SSH server rejected environment variable forwarding via setenv")
						}
						Some(_message) => {
							// Ignore unrelated channel messages and keep waiting for Success/Failure.
						}
						None => bail!("SSH channel closed while sending environment variables"),
					}
				}
			}

			channel
				.exec(true, command)
				.await
				.wrap_err("Failed to execute remote command")?;
			await_channel_confirmation(&mut channel, "remote command exec request").await?;

			stream_channel_output(channel, stdout_tx, stderr_tx, stdin_rx).await
		}
	}
}

/// Streams SSH channel stdout/stderr into local output buffers until exit.
async fn stream_channel_output(
	mut channel: Channel<Msg>,
	stdout_tx: mpsc::Sender<Vec<u8>>,
	stderr_tx: mpsc::Sender<Vec<u8>>,
	mut stdin_rx: Option<mpsc::Receiver<Option<Vec<u8>>>>,
) -> Result<u32> {
	let mut exit_status = None;

	#[expect(
		clippy::integer_division_remainder_used,
		reason = "tokio::select! macro expansion triggers this lint spuriously"
	)]
	loop {
		let recv_stdin = async {
			if let Some(receiver) = stdin_rx.as_mut() {
				Some(receiver.recv().await)
			} else {
				None
			}
		};

		tokio::select! {
			Some(input) = recv_stdin => {
				match input {
					Some(Some(input)) => {
						channel
							.data(input.as_slice())
							.await
							.wrap_err("Failed to forward local stdin to remote command")?;
					}
					Some(None) => {
						channel.eof().await.wrap_err("Failed to send stdin EOF to remote command")?;
						stdin_rx = None;
					}
					None => stdin_rx = None,
				}
			}
			msg = channel.wait() => match msg {
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
				}) => {
					exit_status = Some(status);
					if stdin_rx.is_some() {
						if let Err(error) = channel.eof().await {
							debug!(
								%error,
								"Ignoring stdin EOF send failure after remote command exit"
							);
						}
						stdin_rx = None;
					}
				}
				Some(_) => {}
				None => break,
			}
		}
	}

	exit_status
		.ok_or_else(|| color_eyre::eyre::eyre!("Remote command did not report an exit status"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::types::EnvConfig;
	use crate::env_vars::{EnvForwardMethod, EnvVarRule, EnvVarSelector, EnvVarSpec, EnvVars};
	use crate::ssh::client::auth::AuthenticationFailed;
	use crate::testing::EnvCleanup;
	use color_eyre::eyre::Report;
	use pretty_assertions::assert_eq;
	use serial_test::serial;

	#[test]
	fn report_is_authentication_failure_detects_wrapped_auth_error() {
		let report = Report::from(AuthenticationFailed).wrap_err("Password authentication failed");
		assert!(super::report_is_authentication_failure(&report));
	}

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
				vars: EnvVars::from_rules(vec![EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV"))]),
				forward_method: EnvForwardMethod::Export,
			},
			..Config::default()
		};

		let _cleanup = EnvCleanup::set("NODE_ENV", "development");
		let resolved = resolve_env_vars(
			&config,
			&[EnvVarRule::Spec(EnvVarSpec::value(
				"NODE_ENV",
				"production",
			))],
		)?;
		assert_eq!(
			resolved,
			vec![ResolvedEnvVar {
				name: "NODE_ENV".to_owned(),
				value: "production".to_owned(),
			}]
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn resolve_env_vars_keeps_explicit_cli_value_over_later_cli_pattern() -> Result<()> {
		let config = Config::default();

		let _cleanup = EnvCleanup::set("BIWA_TEST_NODE_ENV", "development");

		let resolved = resolve_env_vars(
			&config,
			&[
				EnvVarRule::Spec(EnvVarSpec::value("BIWA_TEST_NODE_ENV", "production")),
				EnvVarRule::InheritPattern("BIWA_TEST_NODE_*".to_owned()),
			],
		)?;
		assert_eq!(
			resolved,
			vec![ResolvedEnvVar {
				name: "BIWA_TEST_NODE_ENV".to_owned(),
				value: "production".to_owned(),
			}]
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn resolve_env_vars_supports_patterns_and_negation() -> Result<()> {
		let config = Config {
			env: EnvConfig {
				vars: EnvVars::from_rules(vec![
					EnvVarRule::InheritPattern("BIWA_TEST_NODE_*".to_owned()),
					EnvVarRule::Exclude(EnvVarSelector::Pattern("*PATH".to_owned())),
				]),
				forward_method: EnvForwardMethod::Export,
			},
			..Config::default()
		};

		let _cleanup_env = EnvCleanup::set("BIWA_TEST_NODE_ENV", "development");
		let _cleanup_path = EnvCleanup::set("BIWA_TEST_NODE_PATH", "/tmp/biwa-test-node-path");

		let resolved = resolve_env_vars(&config, &[])?;
		assert_eq!(
			resolved,
			vec![ResolvedEnvVar {
				name: "BIWA_TEST_NODE_ENV".to_owned(),
				value: "development".to_owned(),
			}]
		);
		Ok(())
	}
}
