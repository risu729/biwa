use crate::{Result, config::Config};
use clap::Args;
use schemars::schema_for;

/// Generate the configuration schema
#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Schema;

impl Schema {
	pub async fn run(self) -> Result<()> {
		let schema = schema_for!(Config);
		println!("{}", serde_json::to_string_pretty(&schema)?);
		Ok(())
	}
}
