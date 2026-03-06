use crate::{config::types::Config, ssh::exec::execute_command};
use clap::{ArgAction, Parser, Subcommand};
use eyre::bail;
use tracing::Level;

/// Shell completion generation command.
mod completion;
/// Configuration initialization command.
mod init;
/// Command execution on remote hosts.
mod run;
/// Configuration schema generation command.
mod schema;
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
	async fn run(self, config: &Config, quiet: bool, silent: bool) -> eyre::Result<()> {
		match self {
			Self::Run(cmd) => cmd.run(config, quiet, silent).await,
			Self::Init(cmd) => cmd.run(),
			Self::Schema(cmd) => cmd.run(),
			Self::Completion(cmd) => cmd.run(),
			Self::Usage(cmd) => cmd.run(),
		}
	}
}

/// Main entry point for the CLI. Parses arguments and routes to the appropriate command.
pub async fn run() -> eyre::Result<()> {
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
		tracing_subscriber::fmt()
			.pretty()
			.with_max_level(log_level)
			.without_time()
			.init();
	}

	if let Some(command) = cli.command {
		command.run(&config, quiet, silent).await?;
	} else if !cli.run_command_args.is_empty() {
		execute_command(
			&config,
			cli.run_command_args.first().expect("Command is empty"),
			cli.run_command_args.get(1..).expect("Arguments are empty"),
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

	#[test]
	fn cli_run_subcommand() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "run", "ls", "-la"]);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
		assert!(cli.run_command_args.is_empty());
		Ok(())
	}

	#[test]
	fn cli_implicit_run_command() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "ls", "-la"]);
		assert!(cli.command.is_none());
		assert_eq!(cli.run_command_args, vec!["ls", "-la"]);
		Ok(())
	}

	#[test]
	fn cli_verbose() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "-v", "ls"]);
		assert_eq!(cli.verbose, 1);

		let cli = Cli::parse_from(["biwa", "-vv", "ls"]);
		assert_eq!(cli.verbose, 2);

		let cli = Cli::parse_from(["biwa", "-vvv", "ls"]);
		assert_eq!(cli.verbose, 3);
		Ok(())
	}

	#[test]
	fn cli_run_with_verbose() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "-vv", "run", "ls"]);
		assert_eq!(cli.verbose, 2);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
		Ok(())
	}

	#[test]
	fn cli_quiet() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "-q", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.run_command_args, vec!["ls"]);
		Ok(())
	}

	#[test]
	fn cli_quiet_long() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "--quiet", "run", "ls"]);
		assert!(cli.quiet);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
		Ok(())
	}

	#[test]
	fn cli_quiet_with_verbose() -> color_eyre::Result<()> {
		let cli = Cli::parse_from(["biwa", "-q", "-vv", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.verbose, 2);
		Ok(())
	}
}
