use std::io;

use clap::Args;
use clap::builder::PossibleValue;
use eyre::bail;
use strum::EnumString;

/// Generate shell completions.
///
/// Requires the `usage` CLI: <https://usage.jdx.dev>.
#[derive(Args, Debug)]
pub(super) struct Completion {
	/// Shell type to generate completions for.
	shell: Shell,
}

impl Completion {
	/// Run the completion generation logic.
	pub(super) fn run(self) -> eyre::Result<()> {
		let script = self.call_usage()?;
		println!("{}", script.trim());
		Ok(())
	}

	/// Calls usage CLI to generate the shell completion script.
	fn call_usage(&self) -> eyre::Result<String> {
		let shell = self.shell.to_string();
		let result = duct::cmd!(
			"usage",
			"generate",
			"completion",
			&shell,
			"biwa",
			"--usage-cmd",
			"biwa usage"
		)
		.read();

		match result {
			Ok(output) => Ok(output),
			Err(e) if e.kind() == io::ErrorKind::NotFound => {
				bail!(
					"`usage` CLI is required for shell completions but was not found.\n\
					 Install it via mise: `mise use -g usage`\n\
					 Or see: https://usage.jdx.dev/cli/"
				);
			}
			Err(e) => {
				bail!("Failed to execute `usage` command: {e}");
			}
		}
	}
}

/// Supported shell types for completion.
#[derive(Debug, Clone, Copy, EnumString, strum::Display)]
#[strum(serialize_all = "snake_case")]
enum Shell {
	/// Bash shell.
	Bash,
	/// Fish shell.
	Fish,
	/// Zsh shell.
	Zsh,
}

impl clap::ValueEnum for Shell {
	fn value_variants<'a>() -> &'a [Self] {
		&[Self::Bash, Self::Fish, Self::Zsh]
	}

	fn to_possible_value(&self) -> Option<PossibleValue> {
		Some(PossibleValue::new(self.to_string()))
	}
}

#[cfg(test)]
mod tests {
	use crate::cli::{Cli, Commands};
	use clap::Parser as _;

	#[test]
	fn completion_parse_bash() {
		let cli = Cli::parse_from(["biwa", "completion", "bash"]);
		assert!(matches!(cli.command, Some(Commands::Completion(_))));
	}

	#[test]
	fn completion_parse_zsh() {
		let cli = Cli::parse_from(["biwa", "completion", "zsh"]);
		assert!(matches!(cli.command, Some(Commands::Completion(_))));
	}

	#[test]
	fn completion_parse_fish() {
		let cli = Cli::parse_from(["biwa", "completion", "fish"]);
		assert!(matches!(cli.command, Some(Commands::Completion(_))));
	}
}
