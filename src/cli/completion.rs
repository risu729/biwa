use crate::Result;
use crate::cli::Cli;
use std::ffi::OsString;
use usage_rs::complete;

/// Generate shell completion scripts.
#[derive(usage_rs::Args, Debug)]
#[usage(effect = "read")]
pub(super) struct Completion {
	/// Shell type to generate completions for.
	#[usage(value_enum)]
	shell: Shell,
}

impl Completion {
	/// Run the completion generation logic.
	#[expect(
		clippy::unnecessary_wraps,
		reason = "completion subcommand doesn't return Err"
	)]
	pub(super) fn run(self) -> Result<()> {
		let script = Cli::completion_script(self.shell.into());
		println!("{}", script.trim());
		Ok(())
	}
}

/// Answers the hidden shell completion protocol from the compiled usage tables.
///
/// The generated completion scripts call `biwa __complete_word__ …`; this
/// handles that protocol before anything else runs. `argv` excludes the
/// program name.
pub(super) fn completion_request(argv: &[OsString]) -> Option<String> {
	Cli::completion_request(argv)
}

/// Supported shell types for completion.
#[derive(usage_rs::ValueEnum, Debug, Clone, Copy)]
enum Shell {
	/// Bash shell.
	Bash,
	/// Fish shell.
	Fish,
	/// Zsh shell.
	Zsh,
}

impl From<Shell> for complete::Shell {
	#[inline]
	fn from(shell: Shell) -> Self {
		match shell {
			Shell::Bash => Self::Bash,
			Shell::Fish => Self::Fish,
			Shell::Zsh => Self::Zsh,
		}
	}
}

#[cfg(test)]
mod tests {

	use crate::cli::{Cli, Commands};

	#[test]
	fn completion_parse_bash() {
		let cli = Cli::parse_unchecked(["biwa", "completion", "bash"]);
		assert!(matches!(cli.command, Some(Commands::Completion(_))));
	}

	#[test]
	fn completion_parse_zsh() {
		let cli = Cli::parse_unchecked(["biwa", "completion", "zsh"]);
		assert!(matches!(cli.command, Some(Commands::Completion(_))));
	}

	#[test]
	fn completion_parse_fish() {
		let cli = Cli::parse_unchecked(["biwa", "completion", "fish"]);
		assert!(matches!(cli.command, Some(Commands::Completion(_))));
	}

	#[test]
	fn completion_scripts_are_self_contained() {
		use usage_rs::complete;

		for shell in [
			complete::Shell::Bash,
			complete::Shell::Fish,
			complete::Shell::Zsh,
		] {
			let script = Cli::completion_script(shell);
			assert!(script.contains("__complete_word__"), "script: {script}");
		}
	}
}
