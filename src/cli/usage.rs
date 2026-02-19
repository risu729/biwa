use crate::Result;
use crate::cli::Cli;
use clap::{Args, CommandFactory};

/// Generate a usage CLI spec
///
/// See <https://usage.jdx.dev> for more information.
#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Usage;

impl Usage {
	#[allow(clippy::unnecessary_wraps, clippy::unused_self)]
	pub fn run(self) -> Result<()> {
		let cli = Cli::command();
		let spec: usage::Spec = cli.into();
		println!("{}", spec.to_string().trim());
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use crate::cli::Cli;
	use clap::CommandFactory;

	#[test]
	fn test_usage_spec_generation() {
		let cli = Cli::command();
		let spec: usage::Spec = cli.into();
		let output = spec.to_string();
		assert!(!output.is_empty());
		// Should contain the biwa command
		assert!(output.contains("biwa"));
	}
}
