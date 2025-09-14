use crate::{Result, ssh::execute_command};
use clap::Args;

/// Run a command on the CSE server
#[derive(Args, Debug)]
#[clap(visible_alias = "r")]
pub struct Run {
	/// The command to run
	#[arg(required = true)]
	command: String,

	/// The arguments for the command
	#[arg(allow_hyphen_values = true, trailing_var_arg = true)]
	command_args: Vec<String>,
}

impl Run {
	pub async fn run(self) -> Result<()> {
		execute_command(&self.command, &self.command_args).await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use crate::cli::{Cli, Commands};
	use clap::Parser;

	#[test]
	fn test_run_command() {
		let args = Cli::parse_from(["biwa", "run", "ls", "-la"]);
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(run.command, "ls");
			assert_eq!(run.command_args, vec!["-la"]);
		} else {
			panic!("Expected Commands::Run");
		}
	}

	#[test]
	fn test_run_command_alias() {
		let args = Cli::parse_from(["biwa", "r", "pwd"]);
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(run.command, "pwd");
			assert!(run.command_args.is_empty());
		} else {
			panic!("Expected Commands::Run");
		}
	}
}
