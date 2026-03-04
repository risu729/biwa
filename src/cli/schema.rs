use crate::config::types::Config;
use clap::Args;
use schemars::schema_for;

/// Generate the configuration schema.
#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Schema;

impl Schema {
	/// Run the schema generation logic.
	#[expect(clippy::unused_self, reason = "schema subcommand doesn't have flags")]
	pub fn run(self) -> eyre::Result<()> {
		let schema = schema_for!(Config);
		println!("{}", serde_json::to_string_pretty(&schema)?);
		Ok(())
	}
}
