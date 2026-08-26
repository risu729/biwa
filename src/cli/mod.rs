use crate::Result;
use crate::cli::transfer::TransferArgs;
use crate::config::types::Config;
use crate::env_flag;
use ::usage::docs::cli::render_help;
use ::usage::error::UsageErr;
use ::usage::parse::{ParseOutput, Parser as UsageParser};
use color_eyre::eyre::{bail, eyre};
use itertools::Itertools as _;
use std::env;
use std::ffi::OsString;
use std::process;
use tracing::Level;
use tracing_subscriber::{
	filter::Targets, fmt, layer::SubscriberExt as _, registry, util::SubscriberInitExt as _,
};

/// Shell activation and direct command shims.
mod activate;
/// Cleanup of stale remote directories.
mod clean;
/// Shell completion generation command.
mod completion;
/// Configuration initialization command.
mod init;
/// Remote-to-local project mirroring command.
mod pull;
/// Command execution on remote hosts.
mod run;
/// Configuration schema generation command.
mod schema;
/// File synchronization command.
mod sync;
/// Shared project transfer arguments and target resolution.
mod transfer;
/// Usage specification generation command.
mod usage;

/// Parsed CLI arguments.
///
/// Argument definitions live in the usage spec (`biwa.usage.kdl`); this struct
/// is built from the spec parser's output.
#[derive(Debug)]
struct Cli {
	/// The command to run on the remote host.
	command: Option<Commands>,

	/// The arguments for the command to run on the remote host.
	run_command_args: Vec<String>,

	/// The verbosity level (number of `-v` flags).
	verbose: u8,

	/// Suppress biwa internal logs, only showing remote command output.
	quiet: bool,

	/// Suppress all output, including remote command stdout/stderr.
	silent: bool,
}

/// Supported subcommands for the biwa CLI.
#[derive(Debug)]
enum Commands {
	/// Print shell activation code and manage direct command shims.
	Activate(activate::Activate),
	/// Run commands on remote host.
	Run(run::Run),
	/// Push local project files to the remote host.
	Sync(sync::Sync),
	/// Mirror remote project files into the local root.
	///
	/// The remote project is the source of truth. Selected local files and
	/// directories that are absent remotely are deleted.
	Pull(pull::Pull),
	/// Clean stale remote project directories.
	Clean(clean::Clean),
	/// Initialize a biwa configuration file.
	Init(init::Init),
	/// Generate the JSON schema for the configuration.
	Schema(schema::Schema),
	/// Generate shell completion scripts.
	Completion(completion::Completion),
	/// Generate usage command specifications.
	Usage(usage::Usage),
}

/// Result of parsing CLI arguments, including built-in early exits.
enum ParsedCli {
	/// A regular invocation.
	Cli(Cli),
	/// `-h`/`--help` was given; contains the rendered help text.
	Help(String),
	/// `-V`/`--version` was given; contains the version string.
	Version(String),
}

/// Parses an argv (including `argv[0]`) against the usage spec.
fn parse_cli(argv: &[String]) -> Result<ParsedCli> {
	let spec = usage::usage_spec();
	let output = UsageParser::new(spec)
		.explain(argv)
		.map_err(|error| eyre!("{error}"))?;
	for error in &output.errors {
		if let UsageErr::Help(text) = error {
			return Ok(ParsedCli::Help(text.clone()));
		}
		if let UsageErr::Version(version) = error {
			return Ok(ParsedCli::Version(version.clone()));
		}
	}
	if !output.errors.is_empty() {
		bail!(
			"{}",
			output.errors.iter().map(ToString::to_string).join("\n")
		);
	}
	Ok(ParsedCli::Cli(Cli::from_parse_output(&output)?))
}

impl Cli {
	/// Builds the CLI representation from a successful spec parse.
	fn from_parse_output(output: &ParseOutput) -> Result<Self> {
		let command = match output.cmds.get(1).map(|cmd| cmd.name.as_str()) {
			None => None,
			Some("activate") => Some(Commands::Activate(activate::Activate::from_parse(output)?)),
			Some("run") => Some(Commands::Run(run::Run::from_parse(output)?)),
			Some("sync") => Some(Commands::Sync(sync::Sync::from_parse(output))),
			Some("pull") => Some(Commands::Pull(pull::Pull::from_parse(output))),
			Some("clean") => Some(Commands::Clean(clean::Clean::from_parse(output)?)),
			Some("init") => Some(Commands::Init(init::Init::from_parse(output))),
			Some("schema") => Some(Commands::Schema(schema::Schema)),
			Some("completion") => Some(Commands::Completion(completion::Completion::from_parse(
				output,
			)?)),
			Some("usage") => Some(Commands::Usage(usage::Usage)),
			Some(other) => bail!("Unhandled command `{other}` in usage spec"),
		};
		Ok(Self {
			command,
			run_command_args: usage::trailing_arg_values(output, "RUN_COMMAND_ARGS"),
			verbose: usage::flag_count(output, "verbose"),
			quiet: usage::flag_given(output, "quiet"),
			silent: usage::flag_given(output, "silent"),
		})
	}

	/// Parses CLI arguments, returning an error for invalid or early-exit input.
	fn try_parse_from<I: IntoIterator<Item = S>, S: Into<String>>(args: I) -> Result<Self> {
		let words: Vec<String> = args.into_iter().map(Into::into).collect();
		match parse_cli(&words)? {
			ParsedCli::Cli(cli) => Ok(cli),
			ParsedCli::Help(_) => bail!("Unexpected --help in programmatic CLI arguments"),
			ParsedCli::Version(_) => bail!("Unexpected --version in programmatic CLI arguments"),
		}
	}

	/// Parses CLI arguments, panicking on invalid input (test helper).
	#[cfg(test)]
	fn parse_from<I: IntoIterator<Item = S>, S: Into<String>>(args: I) -> Self {
		Self::try_parse_from(args).expect("CLI arguments should parse")
	}
}

/// Converts OS argv entries into UTF-8 strings.
fn argv_strings(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>> {
	args.into_iter()
		.map(|arg| {
			arg.into_string()
				.map_err(|arg| eyre!("Argument {arg:?} is not valid UTF-8"))
		})
		.collect()
}

/// Prints a usage error and exits with the conventional CLI error status.
#[expect(
	clippy::exit,
	reason = "argument parsing failures exit with status 2, matching common CLI conventions"
)]
fn exit_with_usage_error(error: &color_eyre::Report) -> ! {
	eprintln!("{error:#}");
	process::exit(2)
}

/// Main entry point for the CLI. Parses arguments and routes to the appropriate command.
pub async fn run() -> Result<()> {
	let argv = argv_strings(activate::expand_direct_invocation(env::args_os())?)?;
	if argv.len() <= 1 {
		// Bare `biwa` prints help and fails, mirroring Clap's `arg_required_else_help`.
		let spec = usage::usage_spec();
		eprintln!("{}", render_help(spec, &spec.cmd, false));
		#[expect(
			clippy::exit,
			reason = "an argument-less invocation exits with status 2, matching common CLI conventions"
		)]
		process::exit(2);
	}
	let cli = match parse_cli(&argv) {
		Ok(ParsedCli::Cli(cli)) => cli,
		Ok(ParsedCli::Help(text)) => {
			println!("{text}");
			return Ok(());
		}
		Ok(ParsedCli::Version(version)) => {
			println!("{} {version}", usage::usage_spec().bin);
			return Ok(());
		}
		Err(error) => exit_with_usage_error(&error),
	};
	if cli.command.is_none()
		&& let Some(("help", help_command_path)) = cli
			.run_command_args
			.split_first()
			.map(|(first, rest)| (first.as_str(), rest))
	{
		print_help_subcommand(help_command_path)?;
		return Ok(());
	}
	let output_mode = OutputMode::resolve(&cli);
	init_logging(cli.verbose, output_mode);

	match cli.command {
		Some(Commands::Activate(cmd)) => cmd.run()?,
		Some(Commands::Run(cmd)) => cmd.run(output_mode.quiet, output_mode.silent).await?,
		Some(Commands::Sync(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Pull(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Clean(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Init(cmd)) => cmd.run()?,
		Some(Commands::Schema(cmd)) => cmd.run()?,
		Some(Commands::Completion(cmd)) => cmd.run()?,
		Some(Commands::Usage(cmd)) => cmd.run()?,
		None => {
			let (command, command_args) = cli.run_command_args.split_first().ok_or_else(|| {
				eyre!("No command provided. Use `biwa --help` for usage information.")
			})?;
			let config = Config::load()?;
			run::run_remote(
				&config,
				&TransferArgs::default(),
				run::RemoteCommand {
					command,
					command_args,
					cli_env_vars: &[],
				},
				run::RunTransferMode::from_auto(config.sync.auto),
				output_mode.quiet,
				output_mode.silent,
			)
			.await?;
		}
	}

	Ok(())
}

/// Prints long help for the subcommand path given to `biwa help [COMMAND]...`.
fn print_help_subcommand(command_path: &[String]) -> Result<()> {
	let spec = usage::usage_spec();
	let mut command = &spec.cmd;
	for name in command_path {
		command = command
			.find_subcommand(name)
			.ok_or_else(|| eyre!("Unknown command `{name}` for `biwa help`"))?;
	}
	println!("{}", render_help(spec, command, true));
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
	/// Resolves output flags from environment defaults.
	fn from_env() -> Self {
		let silent = env_flag::is_truthy("BIWA_LOG_SILENT");
		let quiet = silent || env_flag::is_truthy("BIWA_LOG_QUIET");
		Self { quiet, silent }
	}

	/// Resolves output flags using CLI precedence over environment defaults.
	fn resolve(cli: &Cli) -> Self {
		let mut mode = Self::from_env();
		mode.silent |= cli.silent;
		mode.quiet = mode.silent || mode.quiet || cli.quiet;
		mode
	}
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
	fn cli_pull_is_a_dedicated_subcommand() {
		let cli = Cli::parse_from(["biwa", "pull"]);
		assert!(matches!(cli.command, Some(Commands::Pull(_))));
		let _pull_on_sync_error = Cli::try_parse_from(["biwa", "sync", "--pull"]).unwrap_err();
	}

	#[test]
	fn cli_push_is_a_sync_alias() {
		let cli = Cli::parse_from(["biwa", "push"]);
		assert!(matches!(cli.command, Some(Commands::Sync(_))));
	}

	#[test]
	fn cli_activate_subcommand() {
		let cli = Cli::parse_from(["biwa", "activate", "--shell", "bash"]);
		assert!(matches!(cli.command, Some(Commands::Activate(_))));
		assert!(cli.run_command_args.is_empty());

		let cli = Cli::parse_from(["biwa", "activate", "doctor"]);
		assert!(matches!(cli.command, Some(Commands::Activate(_))));
		assert!(cli.run_command_args.is_empty());

		let cli = Cli::parse_from(["biwa", "activate", "install", "--force"]);
		assert!(matches!(cli.command, Some(Commands::Activate(_))));
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
