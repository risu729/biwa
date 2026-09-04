use crate::Result;
use crate::cli::transfer::TransferArgs;
use crate::config::types::Config;
use crate::env_flag;
use color_eyre::eyre::eyre;
use std::env;
use std::ffi::{OsStr, OsString};
use std::process;
use tracing::Level;
use tracing_subscriber::{
	filter::Targets, fmt, layer::SubscriberExt as _, registry, util::SubscriberInitExt as _,
};
use usage_rs::embedded::{Exit, Outcome};

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
/// Automated SSH key authentication setup command.
mod setup_ssh;
/// File synchronization command.
mod sync;
/// Shared project transfer arguments and target resolution.
mod transfer;
/// Usage specification generation command.
mod usage;

/// CLI arguments parser.
#[derive(usage_rs::Cli, Debug)]
#[usage(
	name = "biwa",
	bin = "biwa",
	version,
	about,
	arg_required_else_help = true,
	completion = true,
	// Reject mistyped flags instead of forwarding them as the remote command;
	// forwarding positions (`run_command_args`, run's `command_args`) stop
	// flag interpretation before this check applies.
	unknown_flags = "error"
)]
struct Cli {
	/// The command to run on the remote host.
	#[usage(subcommand)]
	command: Option<Commands>,

	/// The arguments for the command to run on the remote host.
	#[usage(double_dash = "automatic", hide = true)]
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
	#[usage(short, long, count, global = true, verbatim_doc_comment)]
	verbose: u8,

	/// Suppress biwa internal logs, only showing remote command output.
	#[usage(short, long, global = true)]
	quiet: bool,

	/// Suppress all output, including remote command stdout/stderr.
	#[usage(short, long, global = true)]
	silent: bool,
}

/// Supported subcommands for the biwa CLI.
///
/// A variant doc comment overrides the wrapped command struct's doc comment
/// in generated help, so the summaries here must stay in sync with the
/// structs' first lines.
#[derive(usage_rs::Subcommands, Debug)]
enum Commands {
	/// Print shell activation code and manage direct command shims.
	Activate(activate::Activate),
	/// Run commands on remote host.
	#[usage(visible_alias = "r")]
	Run(run::Run),
	/// Push local project files to the remote host.
	#[usage(visible_aliases = ["s", "push"])]
	Sync(sync::Sync),
	/// Mirror remote project files into the local root.
	Pull(pull::Pull),
	/// Clean stale remote project directories.
	#[usage(visible_alias = "c")]
	Clean(clean::Clean),
	/// Initialize a biwa configuration file.
	Init(init::Init),
	/// Set up SSH key authentication on the configured host.
	SetupSsh(setup_ssh::SetupSsh),
	/// Generate the JSON schema for the configuration.
	Schema(schema::Schema),
	/// Generate shell completion scripts.
	Completion(completion::Completion),
	/// Generate usage command specifications.
	Usage(usage::Usage),
}

impl Cli {
	/// Parses CLI arguments, returning an error for invalid input.
	fn try_parse_args<I: IntoIterator<Item = S>, S: Into<OsString>>(args: I) -> Result<Self> {
		let words: Vec<OsString> = args.into_iter().map(Into::into).collect();
		let word_refs: Vec<&OsStr> = words.iter().map(OsString::as_os_str).collect();
		Self::parse_from_argv(&word_refs).map_err(|error| {
			eyre!(
				"{}",
				usage_rs::render_failure_plain(
					Self::spec(),
					word_refs.get(1..).unwrap_or_default(),
					&error
				)
			)
		})
	}

	/// Parses CLI arguments, panicking on invalid input (test helper).
	#[cfg(test)]
	fn parse_unchecked<I: IntoIterator<Item = S>, S: Into<OsString>>(args: I) -> Self {
		Self::try_parse_args(args).expect("CLI arguments should parse")
	}
}

/// Prints a usage control-protocol response, then exits.
///
/// Covers spec and completion requests, help and version (stdout, exit 0),
/// and parse failures including `arg_required_else_help` (stderr, exit 2) —
/// the routing and styling decisions come from `usage_rs` itself.
#[expect(
	clippy::exit,
	reason = "argument parsing is the process entry; built-ins and failures exit directly"
)]
fn respond_and_exit(exit: &Exit) -> ! {
	if exit.stderr {
		eprint!("{}", exit.text);
	} else {
		print!("{}", exit.text);
	}
	process::exit(exit.code)
}

/// Main entry point for the CLI. Parses arguments and routes to the appropriate command.
pub async fn run() -> Result<()> {
	let args = activate::expand_direct_invocation(env::args_os())?;
	// `embedded_outcome` answers the hidden spec/completion protocols and
	// renders help, version, and parse failures before any command runs.
	let cli = match Cli::embedded_outcome(args.get(1..).unwrap_or_default()) {
		Outcome::Parsed(cli) => cli,
		Outcome::Exit(exit) => respond_and_exit(&exit),
	};
	let output_mode = OutputMode::resolve(&cli);
	init_logging(cli.verbose, output_mode);

	match cli.command {
		Some(Commands::Activate(cmd)) => cmd.run()?,
		Some(Commands::Run(cmd)) => cmd.run(output_mode.quiet, output_mode.silent).await?,
		Some(Commands::Sync(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Pull(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Clean(cmd)) => cmd.run(output_mode.quiet).await?,
		Some(Commands::Init(cmd)) => cmd.run()?,
		Some(Commands::SetupSsh(cmd)) => cmd.run(output_mode.quiet).await?,
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
		let cli = Cli::parse_unchecked(["biwa", "run", "ls", "-la"]);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
		assert!(cli.run_command_args.is_empty());
	}

	#[test]
	fn cli_pull_is_a_dedicated_subcommand() {
		let cli = Cli::parse_unchecked(["biwa", "pull"]);
		assert!(matches!(cli.command, Some(Commands::Pull(_))));
		let _pull_on_sync_error = Cli::try_parse_args(["biwa", "sync", "--pull"]).unwrap_err();
	}

	#[test]
	fn cli_push_is_a_sync_alias() {
		let cli = Cli::parse_unchecked(["biwa", "push"]);
		assert!(matches!(cli.command, Some(Commands::Sync(_))));
	}

	#[test]
	fn cli_activate_subcommand() {
		let cli = Cli::parse_unchecked(["biwa", "activate", "--shell", "bash"]);
		assert!(matches!(cli.command, Some(Commands::Activate(_))));
		assert!(cli.run_command_args.is_empty());

		let cli = Cli::parse_unchecked(["biwa", "activate", "doctor"]);
		assert!(matches!(cli.command, Some(Commands::Activate(_))));
		assert!(cli.run_command_args.is_empty());

		let cli = Cli::parse_unchecked(["biwa", "activate", "install", "--force"]);
		assert!(matches!(cli.command, Some(Commands::Activate(_))));
		assert!(cli.run_command_args.is_empty());
	}

	#[test]
	fn cli_implicit_run_command() {
		let cli = Cli::parse_unchecked(["biwa", "ls", "-la"]);
		assert!(cli.command.is_none());
		assert_eq!(cli.run_command_args, vec!["ls", "-la"]);
	}

	#[test]
	fn cli_verbose() {
		let cli = Cli::parse_unchecked(["biwa", "-v", "ls"]);
		assert_eq!(cli.verbose, 1);

		let cli = Cli::parse_unchecked(["biwa", "-vv", "ls"]);
		assert_eq!(cli.verbose, 2);

		let cli = Cli::parse_unchecked(["biwa", "-vvv", "ls"]);
		assert_eq!(cli.verbose, 3);
	}

	#[test]
	fn cli_run_with_verbose() {
		let cli = Cli::parse_unchecked(["biwa", "-vv", "run", "ls"]);
		assert_eq!(cli.verbose, 2);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	fn cli_quiet() {
		let cli = Cli::parse_unchecked(["biwa", "-q", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.run_command_args, vec!["ls"]);
	}

	#[test]
	fn cli_quiet_long() {
		let cli = Cli::parse_unchecked(["biwa", "--quiet", "run", "ls"]);
		assert!(cli.quiet);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	fn cli_quiet_with_verbose() {
		let cli = Cli::parse_unchecked(["biwa", "-q", "-vv", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.verbose, 2);
	}

	#[test]
	fn cli_silent() {
		let cli = Cli::parse_unchecked(["biwa", "-s", "ls"]);
		assert!(cli.silent);
		assert!(!cli.quiet);
		assert_eq!(cli.run_command_args, vec!["ls"]);

		let cli = Cli::parse_unchecked(["biwa", "--silent", "run", "ls"]);
		assert!(cli.silent);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	#[serial]
	fn output_mode_silent_flag_implies_quiet() {
		let _quiet_cleanup = EnvCleanup::remove("BIWA_LOG_QUIET");
		let _silent_cleanup = EnvCleanup::remove("BIWA_LOG_SILENT");

		let cli = Cli::parse_unchecked(["biwa", "--silent", "run", "ls"]);
		assert_eq!(
			OutputMode::resolve(&cli),
			OutputMode {
				quiet: true,
				silent: true
			}
		);
	}

	#[test]
	#[serial]
	fn output_mode_quiet_flag_does_not_imply_silent() {
		// `--quiet` only hides biwa's own logs; remote stdout/stderr must survive.
		let _quiet_cleanup = EnvCleanup::remove("BIWA_LOG_QUIET");
		let _silent_cleanup = EnvCleanup::remove("BIWA_LOG_SILENT");

		let cli = Cli::parse_unchecked(["biwa", "--quiet", "run", "ls"]);
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
	fn output_mode_cli_flags_add_to_falsy_env_defaults() {
		// Falsy env values must not veto an explicitly requested flag.
		let _quiet_cleanup = EnvCleanup::set("BIWA_LOG_QUIET", "false");
		let _silent_cleanup = EnvCleanup::set("BIWA_LOG_SILENT", "0");

		let cli = Cli::parse_unchecked(["biwa", "-s", "run", "ls"]);
		assert_eq!(
			OutputMode::resolve(&cli),
			OutputMode {
				quiet: true,
				silent: true
			}
		);
	}

	#[test]
	fn log_level_tracks_verbosity_count() {
		assert_eq!(log_level(0), Level::WARN);
		assert_eq!(log_level(1), Level::INFO);
		assert_eq!(log_level(2), Level::DEBUG);
		assert_eq!(log_level(3), Level::TRACE);
		assert_eq!(log_level(u8::MAX), Level::TRACE);
	}

	#[test]
	#[serial]
	fn output_mode_defaults_to_cli_flags_only_when_env_is_unset() {
		let _quiet_cleanup = EnvCleanup::remove("BIWA_LOG_QUIET");
		let _silent_cleanup = EnvCleanup::remove("BIWA_LOG_SILENT");

		let cli = Cli::parse_unchecked(["biwa", "run", "ls"]);
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

		let cli = Cli::parse_unchecked(["biwa", "run", "ls"]);
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

		let cli = Cli::parse_unchecked(["biwa", "run", "ls"]);
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
