use crate::{config::types::Config, ssh::execute_command};
use clap::Args;

/// Run a command on the CSE server.
#[derive(Args, Debug)]
#[clap(visible_alias = "r")]
pub struct Run {
	/// The command to run.
	#[arg(required = true)]
	command: String,

	/// The arguments for the command.
	#[arg(allow_hyphen_values = true, trailing_var_arg = true)]
	command_args: Vec<String>,
}

impl Run {
	/// Run the execution logic for remote command.
	pub async fn run(self, config: &Config, quiet: bool, silent: bool) -> eyre::Result<()> {
		execute_command(config, &self.command, &self.command_args, quiet, silent).await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use crate::cli::{Cli, Commands};
	use assert_matches::assert_matches;
	use clap::Parser as _;

	#[test]
	fn run_command() {
		let args = Cli::parse_from(["biwa", "run", "ls", "-la"]);
		assert!(args.run_command_args.is_empty());
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(run.command, "ls");
			assert_eq!(run.command_args, vec!["-la"]);
		} else {
			assert_matches!(args.command, Some(Commands::Run(_)));
		}
	}

	#[test]
	fn run_command_alias() {
		let args = Cli::parse_from(["biwa", "r", "pwd"]);
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(run.command, "pwd");
			assert!(run.command_args.is_empty());
		} else {
			assert_matches!(args.command, Some(Commands::Run(_)));
		}
	}
}
