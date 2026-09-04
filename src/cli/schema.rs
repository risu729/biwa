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
		println!("{}", config_schema_json()?);
		Ok(())
	}
}

/// Renders the configuration JSON schema exactly as `biwa schema` prints it.
fn config_schema_json() -> Result<String> {
	let schema = SchemaSettings::default()
		.with_transform(RestrictFormats::default())
		.into_generator()
		.into_root_schema_for::<Config>();
	Ok(serde_json::to_string_pretty(&schema)?)
}

#[cfg(test)]
mod tests {
	use super::config_schema_json;
	use crate::Result;
	use pretty_assertions::assert_eq;

	#[test]
	fn schema_matches_committed_artifact() -> Result<()> {
		// `schema/config.json` is published for editors, so a config struct
		// change must not land without `mise run render:schema`.
		assert_eq!(
			config_schema_json()?.trim(),
			include_str!("../../schema/config.json").trim()
		);
		Ok(())
	}
}
