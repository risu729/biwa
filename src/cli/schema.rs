use crate::Result;
use crate::config::types::Config;
use schemars::{generate::SchemaSettings, transform::RestrictFormats};

/// Generate the JSON schema for the configuration.
#[derive(usage_rs::Args, Debug)]
#[usage(hide = true, effect = "read")]
pub(super) struct Schema;

impl Schema {
	/// Run the schema generation logic.
	#[expect(clippy::unused_self, reason = "schema subcommand doesn't have flags")]
	pub(super) fn run(self) -> Result<()> {
		let schema = SchemaSettings::default()
			.with_transform(RestrictFormats::default())
			.into_generator()
			.into_root_schema_for::<Config>();
		println!("{}", serde_json::to_string_pretty(&schema)?);
		Ok(())
	}
}
