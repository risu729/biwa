use crate::Result;
use crate::{config::types::Config, ssh::exec::execute_command};
use clap::{ArgAction, Parser, Subcommand};
use color_eyre::eyre::bail;
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

impl Commands {
	/// Executes the specific subcommand logic.
	async fn run(self, config: &Config, quiet: bool, silent: bool) -> Result<()> {
		match self {
			Self::Run(cmd) => cmd.run(config, quiet, silent).await,
			Self::Sync(cmd) => cmd.run(config, quiet).await,
			Self::Init(cmd) => cmd.run(),
			Self::Schema(cmd) => cmd.run(),
			Self::Completion(cmd) => cmd.run(),
			Self::Usage(cmd) => cmd.run(),
		}
	}
}

/// Main entry point for the CLI. Parses arguments and routes to the appropriate command.
pub async fn run() -> Result<()> {
	let cli = Cli::parse();

	let config = Config::load()?;
	let silent = cli.silent || config.log.silent;
	let quiet = silent || cli.quiet || config.log.quiet;

	if !quiet {
		let log_level = match cli.verbose {
			0 => Level::WARN,
			1 => Level::INFO,
			2 => Level::DEBUG,
			_ => Level::TRACE,
		};

		let log_targets = Targets::new().with_target("biwa", log_level);
		registry()
			.with(log_targets)
			.with(fmt::layer().pretty().without_time())
			.init();
	}

	if let Some(command) = cli.command {
		command.run(&config, quiet, silent).await?;
	} else if !cli.run_command_args.is_empty() {
		execute_command(
			&config,
			cli.run_command_args.first().expect("Command is empty"),
			cli.run_command_args.get(1..).expect("Arguments are empty"),
			None,
			quiet,
			silent,
		)
		.await?;
	} else {
		bail!("No command provided. Use `biwa --help` for usage information.");
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::sync::Arc;
	use pretty_assertions::assert_eq;
	use std::{io, sync::Mutex};
	use tracing::subscriber;
	use tracing_subscriber::fmt::MakeWriter;

	#[derive(Clone, Default)]
	struct SharedWriter {
		buf: Arc<Mutex<Vec<u8>>>,
	}

	struct SharedGuard {
		buf: Arc<Mutex<Vec<u8>>>,
	}

	impl SharedWriter {
		fn output(&self) -> String {
			let buf = self.buf.lock().expect("buffer lock should succeed");
			String::from_utf8_lossy(&buf).into_owned()
		}
	}

	impl<'a> MakeWriter<'a> for SharedWriter {
		type Writer = SharedGuard;

		fn make_writer(&'a self) -> Self::Writer {
			SharedGuard {
				buf: Arc::clone(&self.buf),
			}
		}
	}

	impl io::Write for SharedGuard {
		fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
			self.buf
				.lock()
				.map_err(|_error| io::Error::other("buffer lock should succeed"))?
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
	fn verbose_filter_only_logs_biwa_targets() {
		let writer = SharedWriter::default();
		let subscriber = registry()
			.with(Targets::new().with_target("biwa", Level::TRACE))
			.with(
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
