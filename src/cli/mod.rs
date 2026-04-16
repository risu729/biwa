use crate::Result;
use crate::cli::sync::SyncArgs;
use crate::config::types::Config;
use clap::{ArgAction, Parser, Subcommand};
use color_eyre::eyre::eyre;
use std::env;
use tracing::Level;
use tracing_subscriber::{
	filter::Targets, fmt, layer::SubscriberExt as _, registry, util::SubscriberInitExt as _,
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

/// Main entry point for the CLI. Parses arguments and routes to the appropriate command.
pub async fn run() -> Result<()> {
	let cli = Cli::parse();
	let output_mode = OutputMode::resolve(&cli);
	init_logging(cli.verbose, output_mode);

	match cli.command {
		Some(Commands::Run(cmd)) => cmd.run(output_mode.quiet, output_mode.silent).await?,
		Some(Commands::Sync(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Init(cmd)) => cmd.run()?,
		Some(Commands::Schema(cmd)) => cmd.run()?,
		Some(Commands::Completion(cmd)) => cmd.run()?,
		Some(Commands::Usage(cmd)) => cmd.run()?,
		None => {
			let (command, args) = cli.run_command_args.split_first().ok_or_else(|| {
				eyre!("No command provided. Use `biwa --help` for usage information.")
			})?;
			let config = Config::load()?;
			run::run_remote(
				&config,
				&SyncArgs::default(),
				run::RemoteCommand {
					command,
					command_args: args,
					cli_env_vars: &[],
				},
				config.sync.auto,
				output_mode.quiet,
				output_mode.silent,
			)
			.await?;
		}
	}

	Ok(())
}

/// Installs tracing subscriber when CLI flags allow internal logs.
fn init_logging(verbose: u8, output_mode: OutputMode) {
	if output_mode.quiet {
		return;
	}

	registry()
		.with(log_targets(verbose))
		.with(fmt::layer().pretty().without_time())
		.init();
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

/// Effective output suppression mode resolved from CLI flags and env vars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputMode {
	/// Suppress biwa internal logs, only showing remote command output.
	quiet: bool,
	/// Suppress all output, including remote command stdout/stderr.
	silent: bool,
}

impl OutputMode {
	/// Resolves output flags using CLI precedence over environment defaults.
	fn resolve(cli: &Cli) -> Self {
		let silent = cli.silent || env_flag_is_truthy("BIWA_LOG_SILENT");
		let quiet = silent || cli.quiet || env_flag_is_truthy("BIWA_LOG_QUIET");
		Self { quiet, silent }
	}
}

/// Returns true when an environment variable is set to a truthy value.
fn env_flag_is_truthy(name: &str) -> bool {
	env::var(name)
		.is_ok_and(|value| {
			matches!(
				value.trim().to_ascii_lowercase().as_str(),
				"1" | "true" | "yes" | "on"
			)
		})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::EnvCleanup;
	use alloc::sync::Arc;
	use pretty_assertions::assert_eq;
	use serial_test::serial;
	use std::io;
	use std::sync::Mutex;
	use tracing::subscriber;
	use tracing_subscriber::fmt::MakeWriter;

	#[derive(Clone, Default)]
	struct TestWriter(Arc<Mutex<Vec<u8>>>);

	struct TestGuard(Arc<Mutex<Vec<u8>>>);

	impl TestWriter {
		fn output(&self) -> String {
			let buf = self.0.lock().expect("test writer lock should succeed");
			String::from_utf8_lossy(&buf).into_owned()
		}
	}

	impl<'a> MakeWriter<'a> for TestWriter {
		type Writer = TestGuard;

		fn make_writer(&'a self) -> Self::Writer {
			TestGuard(Arc::clone(&self.0))
		}
	}

	impl io::Write for TestGuard {
		fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
			self.0
				.lock()
				.map_err(|_e| io::Error::other("failed to acquire test writer lock"))?
				.extend_from_slice(buf);
			Ok(buf.len())
		}

		fn flush(&mut self) -> io::Result<()> {
			Ok(())
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
	#[serial]
	fn output_mode_defaults_to_cli_flags_only_when_env_is_unset() {
		let _quiet_cleanup = EnvCleanup::remove("BIWA_LOG_QUIET");
		let _silent_cleanup = EnvCleanup::remove("BIWA_LOG_SILENT");

		let cli = Cli::parse_from(["biwa", "run", "ls"]);
		assert_eq!(
			OutputMode::resolve(&cli),
			OutputMode {
				quiet: false,
				silent: false
			}
		);
	}

	#[test]
	#[serial]
	fn output_mode_reads_log_env_vars() {
		let _quiet_cleanup = EnvCleanup::set("BIWA_LOG_QUIET", "true");
		let _silent_cleanup = EnvCleanup::set("BIWA_LOG_SILENT", "0");

		let cli = Cli::parse_from(["biwa", "run", "ls"]);
		assert_eq!(
			OutputMode::resolve(&cli),
			OutputMode {
				quiet: true,
				silent: false
			}
		);
	}

	#[test]
	#[serial]
	fn output_mode_silent_env_implies_quiet() {
		let _quiet_cleanup = EnvCleanup::remove("BIWA_LOG_QUIET");
		let _silent_cleanup = EnvCleanup::set("BIWA_LOG_SILENT", "yes");

		let cli = Cli::parse_from(["biwa", "run", "ls"]);
		assert_eq!(
			OutputMode::resolve(&cli),
			OutputMode {
				quiet: true,
				silent: true
			}
		);
	}

	#[test]
	fn verbose_filter_only_logs_biwa_targets() {
		let writer = TestWriter::default();
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
}
