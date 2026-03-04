use crate::{Result, ssh::execute_command};
use clap::{ArgAction, Parser, Subcommand};
use eyre::bail;
use tracing::Level;

mod completion;
mod init;
mod run;
mod schema;
mod usage;

#[derive(Parser, Debug)]
#[command(version, about)]
#[command(arg_required_else_help = true)]
struct Cli {
	/// The command to run on the remote host
	#[command(subcommand)]
	command: Option<Commands>,

	/// The arguments for the command to run on the remote host
	#[arg(allow_hyphen_values = true, trailing_var_arg = true, hide = true)]
	run_command_args: Vec<String>,

	/// Set the verbosity level
	///
	/// Can be used multiple times to increase verbosity (e.g., -v, -vv, -vvv).
	/// By default, only warnings and errors are shown.
	/// -v: info
	/// -vv: debug
	/// -vvv: trace
	#[arg(short, long, action = ArgAction::Count, global = true, verbatim_doc_comment)]
	verbose: u8,

	/// Suppress biwa internal logs, only showing remote command output
	#[arg(short, long, global = true)]
	quiet: bool,

	/// Suppress all output, including remote command stdout/stderr
	#[arg(short, long, global = true)]
	silent: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
	Run(run::Run),
	Init(init::Init),
	Schema(schema::Schema),
	Completion(completion::Completion),
	Usage(usage::Usage),
}

impl Commands {
	pub async fn run(
		self,
		config: &crate::config::Config,
		quiet: bool,
		silent: bool,
	) -> Result<()> {
		match self {
			Self::Run(cmd) => cmd.run(config, quiet, silent).await,
			Self::Init(cmd) => cmd.run(),
			Self::Schema(cmd) => cmd.run(),
			Self::Completion(cmd) => cmd.run(),
			Self::Usage(cmd) => cmd.run(),
		}
	}
}

pub async fn run() -> Result<()> {
	let cli = Cli::parse();

	let config = crate::config::Config::load()?;
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
			&cli.run_command_args[0],
			&cli.run_command_args[1..],
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
	fn test_cli_run_subcommand() {
		let cli = Cli::parse_from(["biwa", "run", "ls", "-la"]);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
		assert!(cli.run_command_args.is_empty());
	}

	#[test]
	fn test_cli_implicit_run_command() {
		let cli = Cli::parse_from(["biwa", "ls", "-la"]);
		assert!(cli.command.is_none());
		assert_eq!(cli.run_command_args, vec!["ls", "-la"]);
	}

	#[test]
	fn test_cli_verbose() {
		let cli = Cli::parse_from(["biwa", "-v", "ls"]);
		assert_eq!(cli.verbose, 1);

		let cli = Cli::parse_from(["biwa", "-vv", "ls"]);
		assert_eq!(cli.verbose, 2);

		let cli = Cli::parse_from(["biwa", "-vvv", "ls"]);
		assert_eq!(cli.verbose, 3);
	}

	#[test]
	fn test_cli_run_with_verbose() {
		let cli = Cli::parse_from(["biwa", "-vv", "run", "ls"]);
		assert_eq!(cli.verbose, 2);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	fn test_cli_quiet() {
		let cli = Cli::parse_from(["biwa", "-q", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.run_command_args, vec!["ls"]);
	}

	#[test]
	fn test_cli_quiet_long() {
		let cli = Cli::parse_from(["biwa", "--quiet", "run", "ls"]);
		assert!(cli.quiet);
		assert!(matches!(cli.command, Some(Commands::Run(_))));
	}

	#[test]
	fn test_cli_quiet_with_verbose() {
		let cli = Cli::parse_from(["biwa", "-q", "-vv", "ls"]);
		assert!(cli.quiet);
		assert_eq!(cli.verbose, 2);
	}
}
