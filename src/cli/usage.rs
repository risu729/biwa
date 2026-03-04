use crate::cli::Cli;
use clap::{Args, CommandFactory as _};

/// Generate a usage CLI spec.
///
/// See <https://usage.jdx.dev> for more information.
#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Usage;

impl Usage {
	#[expect(clippy::unused_self, reason = "usage subcommand doesn't have flags")]
	#[expect(
		clippy::unnecessary_wraps,
		reason = "usage subcommand doesn't return Err"
	)]
	pub fn run(self) -> eyre::Result<()> {
		let cli = Cli::command();
		let spec: usage::Spec = cli.into();
		println!("{}", spec.to_string().trim());
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use crate::cli::Cli;
	use clap::CommandFactory as _;

	#[test]
	fn usage_spec_generation() {
		let cli = Cli::command();
		let spec: usage::Spec = cli.into();
		let output = spec.to_string();
		assert!(!output.is_empty());
		// Should contain the biwa command
		assert!(output.contains("biwa"));
	}
}
