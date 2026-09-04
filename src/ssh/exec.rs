use super::auth::resolve_auth;
use super::sync::shell_quote_path;
use crate::Result;
use crate::config::types::{Config, Umask};
use crate::env_vars::{
	EnvForwardMethod, EnvVarRule, EnvVarSource, is_environment_dependent_env_var,
	local_env_var_names, resolve_env_var_rules,
};
use crate::ssh::client::auth::{AuthenticationFailed, AuthenticationFailureKind};
use crate::ssh::client::execute::{await_channel_confirmation, exit_status_from_signal};
use crate::ssh::client::{Client, HostKeyVerification, HostKeyVerificationFailed};
use crate::ssh::target::ResolvedSshTarget;
use crate::ui::create_spinner;
use bytes::Bytes;
use color_eyre::eyre::{Context as _, Report, bail};
use console::style;
use core::time::Duration;
use indicatif::ProgressBar;
use russh::{Channel, ChannelMsg, Pty, Sig, client::Msg};
use std::env;
use std::io::{
	Error as IoError, ErrorKind as IoErrorKind, IsTerminal as _, Read as _, Result as IoResult,
	stdin as std_stdin,
};
use std::thread;
use tokio::io::{copy, sink, stderr, stdout};
use tokio::signal::ctrl_c;
use tokio::sync::{mpsc, oneshot};
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

/// Returns the fallback policy attached to an authentication failure marker.
fn authentication_failure_kind(report: &Report) -> Option<AuthenticationFailureKind> {
	report
		.downcast_ref::<AuthenticationFailed>()
		.map(|failure| failure.kind())
}

/// Returns true when host-key verification failed and retrying is unsafe.
fn report_is_host_key_verification_failure(report: &Report) -> bool {
	report.downcast_ref::<HostKeyVerificationFailed>().is_some()
}

/// Describes every rejected credential without exposing secrets.
fn authentication_failure_context(
	target: &ResolvedSshTarget,
	rejected_credentials: &[String],
	skipped_agent_identities: usize,
) -> String {
	let attempted = rejected_credentials.join(", ");
	let skipped = if skipped_agent_identities == 0 {
		String::new()
	} else {
		format!(
			"; skipped {skipped_agent_identities} additional agent identities—add an IdentityFile public-key hint to select one"
		)
	};
	format!(
		"Failed to authenticate as {}@{} (attempted: {attempted}{skipped}). Password was not attempted unless ssh.auth = \"password\" was selected",
		target.user, target.hostname
	)
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

/// Terminal interrupt character sent when forwarding Ctrl+C into a remote PTY.
const PTY_INTERRUPT: &[u8] = b"\x03";

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
#[expect(clippy::redundant_pub_crate, reason = "Preferred by reviewer")]
pub(crate) async fn connect(config: &Config, quiet: bool) -> Result<Client> {
	let target = ResolvedSshTarget::resolve(&config.ssh)?;
	let auth_plan = resolve_auth(config, &target).await?;
	let skipped_agent_identities = auth_plan.skipped_agent_identities;
	let mut auth_methods = auth_plan.methods.into_iter();
	let mut auth_method = auth_methods
		.next()
		.expect("authentication resolution must return at least one method");
	let mut rejected_credentials = Vec::new();
	let spinner = if quiet {
		None
	} else {
		Some(create_spinner(format!(
			"Connecting to {}@{}:{}...",
			target.user, target.hostname, target.port
		)))
	};

	let _spinner_cleanup = SpinnerGuard(spinner);

	let mut retries = 3_usize;
	let mut delay = Duration::from_millis(500);

	let client = loop {
		let verification = HostKeyVerification::new(
			target.hostname.clone(),
			target.port,
			config.ssh.host_key_checking,
			config.ssh.known_hosts.clone(),
		);
		match Client::connect(
			(target.hostname.as_str(), target.port),
			target.user.as_str(),
			auth_method.clone(),
			verification,
		)
		.await
		{
			Ok(c) => break c,
			Err(e)
				if authentication_failure_kind(&e)
					== Some(AuthenticationFailureKind::Retryable) =>
			{
				debug!(
					credential = %auth_method.description(),
					error = %e,
					"SSH authentication candidate failed"
				);
				rejected_credentials.push(auth_method.description());
				if let Some(fallback) = auth_methods.next() {
					info!("Authentication failed; trying the next public-key candidate");
					auth_method = fallback;
					continue;
				}
				return Err(e).wrap_err_with(|| {
					authentication_failure_context(
						&target,
						&rejected_credentials,
						skipped_agent_identities,
					)
				});
			}
			Err(e)
				if authentication_failure_kind(&e) == Some(AuthenticationFailureKind::Terminal) =>
			{
				return Err(e);
			}
			Err(e) if report_is_host_key_verification_failure(&e) => return Err(e),
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
						target.user, target.hostname, target.port
					)
				});
			}
		}
	};

	info!(
		host = %target.hostname,
		port = target.port,
		user = %target.user,
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
				Err(error) if error.kind() == IoErrorKind::Interrupted => {
					debug!("Retrying local stdin read interrupted by signal");
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

/// Local Ctrl+C notifications for a single remote command.
struct CtrlCForwarder {
	/// Receiver notified whenever local Ctrl+C is observed.
	interrupt_rx: mpsc::Receiver<IoResult<()>>,
	/// Stops the background listener when command streaming ends.
	shutdown_tx: Option<oneshot::Sender<()>>,
}

impl CtrlCForwarder {
	/// Spawns a listener that converts local Ctrl+C signals into channel notifications.
	fn spawn() -> Self {
		let (interrupt_tx, interrupt_rx) = mpsc::channel(8);
		let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

		tokio::spawn(async move {
			#[expect(
				clippy::integer_division_remainder_used,
				reason = "tokio::select! macro expansion triggers this lint spuriously"
			)]
			loop {
				tokio::select! {
					result = ctrl_c() => {
						if interrupt_tx.send(result).await.is_err() {
							break;
						}
					}
					_ = &mut shutdown_rx => break,
				}
			}
		});

		Self {
			interrupt_rx,
			shutdown_tx: Some(shutdown_tx),
		}
	}

	/// Receives the next local Ctrl+C notification.
	async fn recv(&mut self) -> Option<IoResult<()>> {
		self.interrupt_rx.recv().await
	}
}

impl Drop for CtrlCForwarder {
	fn drop(&mut self) {
		if let Some(shutdown_tx) = self.shutdown_tx.take()
			&& shutdown_tx.send(()).is_err()
		{
			debug!("Ctrl+C listener already stopped");
		}
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

	Ok(exit_status)
}

/// Options for remote command execution.
pub struct ExecuteCommandOptions<'a> {
	/// The command to run.
	pub command: &'a str,
	/// The arguments for the command.
	pub args: &'a [String],
	/// CLI-provided environment variables.
	pub cli_env_vars: &'a [EnvVarRule],
	/// Remote working directory to enter before running the command.
	pub working_dir: Option<&'a str>,
	/// Suppresses local progress output when true.
	pub quiet: bool,
	/// Suppresses forwarded remote stdout/stderr when true.
	pub silent: bool,
}

/// Execute a command and return any confirmed remote exit status without
/// converting a non-zero status into a transport error.
pub async fn execute_command_status(
	client: &Client,
	config: &Config,
	options: ExecuteCommandOptions<'_>,
) -> Result<u32> {
	info!(
		command = options.command,
		args_count = options.args.len(),
		has_working_dir = options.working_dir.is_some(),
		quiet = options.quiet,
		silent = options.silent,
		"Starting remote command execution"
	);
	let full_command = build_command(options.command, options.args);
	let env_vars = resolve_env_vars(config, options.cli_env_vars)?;
	run_command(
		client,
		&full_command,
		&env_vars,
		&config.env.forward_method,
		RunCommandOptions {
			working_dir: options.working_dir,
			umask: &config.ssh.umask,
			quiet: options.quiet,
			silent: options.silent,
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
	match forward_method {
		EnvForwardMethod::Export => {
			let mut channel = client
				.get_channel()
				.await
				.wrap_err("Failed to open SSH session channel")?;

			if streams.stdin_is_terminal {
				request_terminal_pty(&mut channel).await?;
			}

			channel
				.exec(true, command)
				.await
				.wrap_err("Failed to execute remote command")?;
			await_channel_confirmation(&mut channel, "remote command exec request").await?;

			stream_channel_output(channel, streams).await
		}
		EnvForwardMethod::Setenv => {
			let mut channel = client
				.get_channel()
				.await
				.wrap_err("Failed to open SSH session channel")?;

			if streams.stdin_is_terminal {
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

			stream_channel_output(channel, streams).await
		}
	}
}

/// Stops forwarding stdin once the remote side has reported process termination.
async fn stop_forwarding_stdin_after_remote_exit(
	channel: &Channel<Msg>,
	stdin_rx: &mut Option<StdinReceiver>,
) {
	if stdin_rx.is_some() {
		if let Err(error) = channel.eof().await {
			debug!(
				%error,
				"Ignoring stdin EOF send failure after remote command exit"
			);
		}
		*stdin_rx = None;
	}
}

/// Forwards a local Ctrl+C into the remote session.
async fn forward_local_sigint(channel: &Channel<Msg>, stdin_is_terminal: bool) -> Result<()> {
	if stdin_is_terminal {
		channel
			.data(PTY_INTERRUPT)
			.await
			.wrap_err("Failed to forward Ctrl+C to remote PTY")?;
	} else {
		channel
			.signal(Sig::INT)
			.await
			.wrap_err("Failed to send SIGINT to remote command")?;
	}

	debug!(
		remote_pty = stdin_is_terminal,
		"Forwarded local Ctrl+C to remote command"
	);

	Ok(())
}

/// Streams SSH channel stdout/stderr into local output buffers until exit.
async fn stream_channel_output(
	mut channel: Channel<Msg>,
	streams: ExecuteCommandStreams,
) -> Result<u32> {
	let ExecuteCommandStreams {
		stdout_tx,
		stderr_tx,
		mut stdin_rx,
		stdin_is_terminal,
	} = streams;
	let mut exit_status = None;
	let mut ctrl_c_forwarder = CtrlCForwarder::spawn();

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
			},
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
					stop_forwarding_stdin_after_remote_exit(&channel, &mut stdin_rx).await;
				}
				Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
					exit_status = Some(exit_status_from_signal(&signal_name));
					stop_forwarding_stdin_after_remote_exit(&channel, &mut stdin_rx).await;
				}
				Some(_) => {}
				None => break,
			},
			interrupt = ctrl_c_forwarder.recv(), if exit_status.is_none() => match interrupt {
				Some(result) => {
					result.wrap_err("Failed to listen for local Ctrl+C")?;
					forward_local_sigint(&channel, stdin_is_terminal).await?;
				}
				None => bail!("Local Ctrl+C listener stopped before remote command completed"),
			},
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
	use crate::ssh::client::auth::{AuthenticationFailed, AuthenticationFailureKind};
	use crate::testing::EnvCleanup;
	use color_eyre::eyre::Report;
	use pretty_assertions::assert_eq;
	use serial_test::serial;

	#[test]
	fn authentication_failure_kind_detects_wrapped_auth_errors() {
		let retryable = Report::from(AuthenticationFailed::retryable())
			.wrap_err("Password authentication failed");
		let terminal = Report::from(AuthenticationFailed::terminal())
			.wrap_err("MFA continuation is unsupported");

		assert_eq!(
			super::authentication_failure_kind(&retryable),
			Some(AuthenticationFailureKind::Retryable)
		);
		assert_eq!(
			super::authentication_failure_kind(&terminal),
			Some(AuthenticationFailureKind::Terminal)
		);
	}

	#[test]
	fn report_is_host_key_failure_detects_structured_error() {
		let report = Report::new(super::HostKeyVerificationFailed::new("host key rejected"));

		assert!(super::report_is_host_key_verification_failure(&report));
		assert_eq!(super::authentication_failure_kind(&report), None);
	}

	#[test]
	fn authentication_failure_context_lists_attempts_without_skipped_keys() {
		let target = ResolvedSshTarget {
			lookup_host: "alias".to_owned(),
			hostname: "example.test".to_owned(),
			port: 22,
			user: "alice".to_owned(),
			identity_files: Vec::new(),
		};
		assert_eq!(
			authentication_failure_context(
				&target,
				&[
					"agent key SHA256:first".to_owned(),
					"key ~/.ssh/id_ed25519".to_owned()
				],
				0,
			),
			"Failed to authenticate as alice@example.test (attempted: agent key SHA256:first, key ~/.ssh/id_ed25519). Password was not attempted unless ssh.auth = \"password\" was selected"
		);
	}

	#[test]
	fn authentication_failure_context_reports_skipped_agent_keys() {
		let target = ResolvedSshTarget {
			lookup_host: "alias".to_owned(),
			hostname: "example.test".to_owned(),
			port: 22,
			user: "alice".to_owned(),
			identity_files: Vec::new(),
		};
		let message =
			authentication_failure_context(&target, &["agent key SHA256:first".to_owned()], 3);
		assert!(message.contains("skipped 3 additional agent identities"));
		assert!(message.contains("IdentityFile public-key hint"));
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
	fn resolve_env_vars_reports_a_missing_inherited_variable() {
		// An exact inherit rule is kept even when the variable is absent, so the
		// failure has to name the variable instead of silently forwarding nothing.
		let _cleanup = EnvCleanup::remove("BIWA_TEST_ABSENT");

		let error = resolve_env_vars(
			&Config::default(),
			&[EnvVarRule::Spec(EnvVarSpec::inherit("BIWA_TEST_ABSENT"))],
		)
		.expect_err("an absent inherited variable must fail loudly");

		assert_eq!(
			error.to_string(),
			"Environment variable `BIWA_TEST_ABSENT` is not set locally"
		);
	}

	#[serial]
	#[test]
	fn local_terminal_type_falls_back_to_xterm() {
		let _cleanup = EnvCleanup::set("TERM", "screen-256color");
		assert_eq!(local_terminal_type(), "screen-256color");

		let _blank = EnvCleanup::set("TERM", "   ");
		assert_eq!(local_terminal_type(), "xterm");

		let _missing = EnvCleanup::remove("TERM");
		assert_eq!(local_terminal_type(), "xterm");
	}

	#[serial]
	#[test]
	fn terminal_dimension_rejects_unusable_values() {
		// A zero or unparsable dimension would make the PTY request meaningless,
		// so only positive integers may override the default.
		let _cleanup = EnvCleanup::set("BIWA_TEST_COLUMNS", "120");
		assert_eq!(terminal_dimension("BIWA_TEST_COLUMNS", 80), 120);

		for unusable in ["0", "-1", "wide", ""] {
			let _unusable = EnvCleanup::set("BIWA_TEST_COLUMNS", unusable);
			assert_eq!(terminal_dimension("BIWA_TEST_COLUMNS", 80), 80);
		}

		let _missing = EnvCleanup::remove("BIWA_TEST_COLUMNS");
		assert_eq!(terminal_dimension("BIWA_TEST_COLUMNS", 24), 24);
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
