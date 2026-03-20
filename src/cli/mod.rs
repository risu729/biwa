use crate::Result;
use crate::cli::sync::SyncArgs;
use crate::config::types::Config;
use alloc::sync::Arc;
use clap::{ArgAction, Parser, Subcommand};
use color_eyre::eyre::{bail, eyre};
use core::mem;
use std::io;
use std::sync::Mutex;
use tracing::{Level, subscriber};
use tracing_subscriber::{
	filter::Targets, fmt, fmt::MakeWriter, layer::SubscriberExt as _, registry,
	util::SubscriberInitExt as _,
};

/// Shell completion generation command.
mod completion;
/// Configuration initialization command.
mod init;
/// Command execution on remote hosts.
mod run;
/// Configuration schema generation command.
mod schema;
/// File synchronization command.
mod sync;
/// Usage specification generation command.
mod usage;

/// CLI arguments parser.
#[derive(Parser, Debug)]
#[command(version, about)]
#[command(arg_required_else_help = true)]
struct Cli {
	/// The command to run on the remote host.
	#[command(subcommand)]
	command: Option<Commands>,

	/// The arguments for the command to run on the remote host.
	#[arg(allow_hyphen_values = true, trailing_var_arg = true, hide = true)]
	run_command_args: Vec<String>,

	/// Set the verbosity level.
	///
	/// Can be used multiple times to increase verbosity (e.g., -v, -vv, -vvv).
	/// By default, only warnings and errors are shown.
	/// -v: info
	/// -vv: debug
	/// -vvv: trace
	#[expect(
		clippy::doc_paragraphs_missing_punctuation,
		reason = "no need to add period after the list of options"
	)]
	#[arg(short, long, action = ArgAction::Count, global = true, verbatim_doc_comment)]
	verbose: u8,

	/// Suppress biwa internal logs, only showing remote command output.
	#[arg(short, long, global = true)]
	quiet: bool,

	/// Suppress all output, including remote command stdout/stderr.
	#[arg(short, long, global = true)]
	silent: bool,
}

/// Supported subcommands for the biwa CLI.
#[derive(Subcommand, Debug)]
enum Commands {
	/// Run commands on remote host.
	Run(run::Run),
	/// Synchronize files to remote host.
	Sync(sync::Sync),
	/// Initialize a biwa configuration file.
	Init(init::Init),
	/// Generate the JSON schema for the configuration.
	Schema(schema::Schema),
	/// Generate shell completion scripts.
	Completion(completion::Completion),
	/// Generate usage command specifications.
	Usage(usage::Usage),
}

impl Commands {
	/// Returns whether this subcommand needs runtime configuration loading.
	const fn needs_config(&self) -> bool {
		matches!(self, Self::Run(_) | Self::Sync(_))
	}
}

/// Main entry point for the CLI. Parses arguments and routes to the appropriate command.
pub async fn run() -> Result<()> {
	let cli = Cli::parse();

	if let Some(command) = cli.command.as_ref() {
		if command.needs_config() {
			let (config, quiet, silent) =
				load_config_with_buffered_logs(&cli, &mut io::stderr().lock())?;

			if !quiet {
				registry()
					.with(log_targets(cli.verbose))
					.with(fmt::layer().pretty().without_time())
					.init();
			}

			match cli.command.expect("command presence already checked") {
				Commands::Run(cmd) => cmd.run(&config, quiet, silent).await?,
				Commands::Sync(cmd) => cmd.run(&config, quiet).await?,
				Commands::Init(_)
				| Commands::Schema(_)
				| Commands::Completion(_)
				| Commands::Usage(_) => {
					bail!("Internal error: config-free command reached config-dependent path");
				}
			}
		} else {
			match cli.command.expect("command presence already checked") {
				Commands::Init(cmd) => cmd.run()?,
				Commands::Schema(cmd) => cmd.run()?,
				Commands::Completion(cmd) => cmd.run()?,
				Commands::Usage(cmd) => cmd.run()?,
				Commands::Run(_) | Commands::Sync(_) => {
					bail!("Internal error: config-dependent command reached config-free path");
				}
			}
		}
	} else if !cli.run_command_args.is_empty() {
		let (config, quiet, silent) =
			load_config_with_buffered_logs(&cli, &mut io::stderr().lock())?;

		if !quiet {
			registry()
				.with(log_targets(cli.verbose))
				.with(fmt::layer().pretty().without_time())
				.init();
		}

		let (command, args) = cli.run_command_args.split_first().ok_or_else(|| {
			eyre!("No command provided. Use `biwa --help` for usage information.")
		})?;
		run::run_remote(
			&config,
			&SyncArgs::default(),
			run::RemoteCommand {
				command,
				command_args: args,
				cli_env_vars: &[],
			},
			config.sync.auto,
			quiet,
			silent,
		)
		.await?;
	} else {
		bail!("No command provided. Use `biwa --help` for usage information.");
	}
	Ok(())
}

/// Loads configuration while buffering any logs emitted before the global subscriber is ready.
fn load_config_with_buffered_logs(
	cli: &Cli,
	stderr: &mut impl io::Write,
) -> Result<(Config, bool, bool)> {
	if cli.silent || cli.quiet {
		let config = Config::load()?;
		let silent = cli.silent || config.log.silent;
		let quiet = silent || cli.quiet || config.log.quiet;
		return Ok((config, quiet, silent));
	}

	let writer = BufferedWriter::default();
	let load_subscriber = registry().with(log_targets(cli.verbose)).with(
		fmt::layer()
			.pretty()
			.without_time()
			.with_ansi(false)
			.with_writer(writer.clone()),
	);

	let config_result = subscriber::with_default(load_subscriber, Config::load);
	if config_result
		.as_ref()
		.map_or(true, |config| !(config.log.silent || config.log.quiet))
	{
		writer.write_to(stderr)?;
	}

	let config = config_result?;
	let silent = cli.silent || config.log.silent;
	let quiet = silent || cli.quiet || config.log.quiet;
	Ok((config, quiet, silent))
}

/// Returns the log level selected by CLI verbosity flags.
const fn log_level(verbose: u8) -> Level {
	match verbose {
		0 => Level::WARN,
		1 => Level::INFO,
		2 => Level::DEBUG,
		_ => Level::TRACE,
	}
}

/// Returns the target filter used for internal biwa logs.
fn log_targets(verbose: u8) -> Targets {
	Targets::new().with_target("biwa", log_level(verbose))
}

/// Shared in-memory writer for buffering load-phase logs.
#[derive(Clone, Default)]
struct BufferedWriter {
	/// Shared byte buffer receiving formatted tracing output.
	buf: Arc<Mutex<Vec<u8>>>,
}

/// Write guard that appends tracing output into the shared in-memory buffer.
struct BufferedGuard {
	/// Shared byte buffer receiving formatted tracing output.
	buf: Arc<Mutex<Vec<u8>>>,
}

impl BufferedWriter {
	#[cfg(test)]
	fn output(&self) -> String {
		let buf = self.buf.lock().expect("buffer lock should succeed");
		String::from_utf8_lossy(&buf).into_owned()
	}

	/// Flushes the buffered tracing output into the provided writer.
	fn write_to(&self, writer: &mut impl io::Write) -> io::Result<()> {
		let bytes = {
			let mut buf = match self.buf.lock() {
				Ok(buf) => buf,
				Err(_e) => return Ok(()),
			};
			mem::take(&mut *buf)
		};

		if !bytes.is_empty() {
			if let Err(error) = writer.write_all(&bytes) {
				if error.kind() != io::ErrorKind::BrokenPipe {
					return Err(error);
				}
				return Ok(());
			}

			if let Err(error) = writer.flush()
				&& error.kind() != io::ErrorKind::BrokenPipe
			{
				return Err(error);
			}
		}

		Ok(())
	}
}

impl<'a> MakeWriter<'a> for BufferedWriter {
	type Writer = BufferedGuard;

	fn make_writer(&'a self) -> Self::Writer {
		BufferedGuard {
			buf: Arc::clone(&self.buf),
		}
	}
}

impl io::Write for BufferedGuard {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.buf
			.lock()
			.map_err(|_e| io::Error::other("failed to acquire buffer lock"))?
			.extend_from_slice(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use serial_test::serial;
	use std::path::{Path, PathBuf};
	use std::{env, fs};
	use tempfile::tempdir;

	struct CurrentDirGuard {
		original_dir: PathBuf,
	}

	impl CurrentDirGuard {
		fn new(path: &Path) -> Result<Self> {
			let original_dir = env::current_dir()?;
			env::set_current_dir(path)?;
			Ok(Self { original_dir })
		}
	}

	impl Drop for CurrentDirGuard {
		fn drop(&mut self) {
			let _result = env::set_current_dir(&self.original_dir);
		}
	}

	struct BrokenPipeWriter;

	impl io::Write for BrokenPipeWriter {
		fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
			Err(io::Error::from(io::ErrorKind::BrokenPipe))
		}

		fn flush(&mut self) -> io::Result<()> {
			Err(io::Error::from(io::ErrorKind::BrokenPipe))
		}
	}

	#[test]
	fn cli_run_subcommand() {
		let cli = Cli::parse_from(["biwa", "run", "ls", "-la"]);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
		assert!(cli.run_command_args.is_empty());
	}

	#[test]
	fn cli_implicit_run_command() {
		let cli = Cli::parse_from(["biwa", "ls", "-la"]);
		assert!(cli.command.is_none());
		assert_eq!(cli.run_command_args, vec!["ls", "-la"]);
	}

	#[test]
	fn cli_verbose() {
		let cli = Cli::parse_from(["biwa", "-v", "ls"]);
		assert_eq!(cli.verbose, 1);

		let cli = Cli::parse_from(["biwa", "-vv", "ls"]);
		assert_eq!(cli.verbose, 2);

		let cli = Cli::parse_from(["biwa", "-vvv", "ls"]);
		assert_eq!(cli.verbose, 3);
	}

	#[test]
	fn cli_run_with_verbose() {
		let cli = Cli::parse_from(["biwa", "-vv", "run", "ls"]);
		assert_eq!(cli.verbose, 2);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	fn cli_quiet() {
		let cli = Cli::parse_from(["biwa", "-q", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.run_command_args, vec!["ls"]);
	}

	#[test]
	fn cli_quiet_long() {
		let cli = Cli::parse_from(["biwa", "--quiet", "run", "ls"]);
		assert!(cli.quiet);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	fn cli_quiet_with_verbose() {
		let cli = Cli::parse_from(["biwa", "-q", "-vv", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.verbose, 2);
	}

	#[test]
	fn schema_and_usage_do_not_require_runtime_config() {
		let schema = Cli::parse_from(["biwa", "schema"]);
		let usage = Cli::parse_from(["biwa", "usage"]);
		let init = Cli::parse_from(["biwa", "init"]);
		let completion = Cli::parse_from(["biwa", "completion", "bash"]);

		assert!(matches!(schema.command.as_ref(), Some(command) if !command.needs_config()));
		assert!(matches!(usage.command.as_ref(), Some(command) if !command.needs_config()));
		assert!(matches!(init.command.as_ref(), Some(command) if !command.needs_config()));
		assert!(matches!(completion.command.as_ref(), Some(command) if !command.needs_config()));
	}

	#[test]
	fn verbose_filter_only_logs_biwa_targets() {
		let writer = BufferedWriter::default();
		let subscriber = registry().with(log_targets(3)).with(
			fmt::layer()
				.with_ansi(false)
				.without_time()
				.with_writer(writer.clone()),
		);

		subscriber::with_default(subscriber, || {
			tracing::info!(target: "biwa::cli::tests", "biwa-target-log");
			tracing::info!(target: "dependency::tests", "dependency-target-log");
		});

		let output = writer.output();
		assert!(output.contains("biwa-target-log"), "logs were: {output}");
		assert!(
			!output.contains("dependency-target-log"),
			"logs were: {output}"
		);
	}

	#[serial]
	#[test]
	fn buffered_config_logs_are_flushed_when_logging_is_enabled() -> Result<()> {
		let dir = tempdir()?;
		fs::write(
			dir.path().join("biwa.toml"),
			"[ssh]\nhost = \"example.test\"\nuser = \"testuser\"\n[sync]\nremote_root = \"/absolute/path\"\n",
		)?;

		let _dir_guard = CurrentDirGuard::new(dir.path())?;

		let cli = Cli {
			command: None,
			run_command_args: Vec::new(),
			verbose: 0,
			quiet: false,
			silent: false,
		};
		let mut stderr = Vec::new();
		let result = load_config_with_buffered_logs(&cli, &mut stderr);

		let (_config, quiet, silent) = result?;
		assert!(!quiet);
		assert!(!silent);

		let output = String::from_utf8(stderr)?;
		assert!(output.contains("Absolute remote_root path detected"));
		Ok(())
	}

	#[serial]
	#[test]
	fn buffered_config_logs_respect_loaded_quiet_mode() -> Result<()> {
		let dir = tempdir()?;
		fs::write(
			dir.path().join("biwa.toml"),
			"[ssh]\nhost = \"example.test\"\nuser = \"testuser\"\n[log]\nquiet = true\n[sync]\nremote_root = \"/absolute/path\"\n",
		)?;

		let _dir_guard = CurrentDirGuard::new(dir.path())?;

		let cli = Cli {
			command: None,
			run_command_args: Vec::new(),
			verbose: 0,
			quiet: false,
			silent: false,
		};
		let mut stderr = Vec::new();
		let result = load_config_with_buffered_logs(&cli, &mut stderr);

		let (_config, quiet, silent) = result?;
		assert!(quiet);
		assert!(!silent);
		assert!(stderr.is_empty(), "logs were: {stderr:?}");
		Ok(())
	}

	#[serial]
	#[test]
	fn buffered_config_logs_short_circuit_when_cli_quiet_is_enabled() -> Result<()> {
		let dir = tempdir()?;
		fs::write(
			dir.path().join("biwa.toml"),
			"[ssh]\nhost = \"example.test\"\nuser = \"testuser\"\n[sync]\nremote_root = \"/absolute/path\"\n",
		)?;

		let _dir_guard = CurrentDirGuard::new(dir.path())?;

		let cli = Cli {
			command: None,
			run_command_args: Vec::new(),
			verbose: 0,
			quiet: true,
			silent: false,
		};
		let mut stderr = Vec::new();
		let (_config, quiet, silent) = load_config_with_buffered_logs(&cli, &mut stderr)?;

		assert!(quiet);
		assert!(!silent);
		assert!(stderr.is_empty(), "logs were: {stderr:?}");
		Ok(())
	}

	#[serial]
	#[test]
	fn buffered_config_logs_are_flushed_when_loading_fails() -> Result<()> {
		let dir = tempdir()?;
		fs::write(dir.path().join("biwa.toml"), "[sync]\nremote_root = [\n")?;

		let _dir_guard = CurrentDirGuard::new(dir.path())?;

		let cli = Cli {
			command: None,
			run_command_args: Vec::new(),
			verbose: 2,
			quiet: false,
			silent: false,
		};
		let mut stderr = Vec::new();
		let result = load_config_with_buffered_logs(&cli, &mut stderr);

		let _error = result.expect_err("loading invalid config should fail");

		let output = String::from_utf8(stderr)?;
		assert!(
			output.contains("Loading configuration"),
			"logs were: {output}"
		);
		Ok(())
	}

	#[test]
	fn buffered_writer_ignores_broken_pipe() -> Result<()> {
		let writer = BufferedWriter::default();
		let subscriber = registry().with(log_targets(0)).with(
			fmt::layer()
				.with_ansi(false)
				.without_time()
				.with_writer(writer.clone()),
		);

		subscriber::with_default(subscriber, || {
			tracing::warn!(target: "biwa::cli::tests", "buffered warning");
		});

		let mut broken_pipe = BrokenPipeWriter;
		writer.write_to(&mut broken_pipe)?;
		Ok(())
	}
}
