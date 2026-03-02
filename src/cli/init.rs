use crate::{Result, config::format::ConfigFormat, config::types::Config};
use clap::Args;
use eyre::bail;
use serde_json::json;
use std::fs;
use std::path::Path;

/// Initialize a new configuration file
#[derive(Args, Debug)]
pub struct Init {
	/// Force overwrite if file exists
	#[arg(long, short)]
	force: bool,

	/// Format to generate (toml, json, jsonc, json5, yaml, yml)
	#[arg(long, default_value = "toml")]
	format: String,
}

impl Init {
	pub fn run(self) -> Result<()> {
		let (filename, content) = self.generate()?;
		let path = Path::new(&filename);
		if path.exists() && !self.force {
			bail!("{} already exists. Use --force to overwrite.", filename);
		}

		fs::write(path, content)?;
		eprintln!("Created {filename}");
		Ok(())
	}

	fn generate(&self) -> Result<(String, String)> {
		let filename = format!("biwa.{}", self.format.to_ascii_lowercase());
		let schema_url = "https://biwa.takuk.me/schema/config.json";

		let format = ConfigFormat::from_extension(&self.format)
			.ok_or_else(|| eyre::eyre!("Unsupported format: {}", self.format))?;

		let content = match format {
			ConfigFormat::Toml => {
				let template = Config::template(format);
				format!("#:schema {schema_url}\n\n{template}")
			}
			ConfigFormat::Yaml => {
				let template = Config::template(format);
				format!("# yaml-language-server: $schema={schema_url}\n{template}")
			}
			ConfigFormat::Json => {
				// For strict JSON, revert to serde_json-based generation to ensure validity.
				let config = Config::default();
				let mut value = serde_json::to_value(&config)?;
				if let Some(obj) = value.as_object_mut() {
					obj.insert("$schema".to_string(), json!(schema_url));
				}
				serde_json::to_string_pretty(&value)?
			}
			ConfigFormat::Json5 => {
				// Distinguish json5 vs jsonc by the requested extension.
				if self.format.eq_ignore_ascii_case("jsonc") {
					// JSONC should be strict JSON (quoted keys, no trailing commas),
					// but tools treat .jsonc as JSON-with-comments, so emitting valid JSON
					// is always safe.
					let config = Config::default();
					let mut value = serde_json::to_value(&config)?;
					if let Some(obj) = value.as_object_mut() {
						obj.insert("$schema".to_string(), json!(schema_url));
					}
					serde_json::to_string_pretty(&value)?
				} else {
					// For JSON5, keep using the confique template (with comments etc.),
					// but inject a JSON5-friendly $schema field at the top.
					let template = Config::template(format);
					let body = template.lines().skip(1).collect::<Vec<_>>().join("\n");
					format!("{{\n  $schema: \"{schema_url}\",\n{body}")
				}
			}
		};

		Ok((filename, content))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use insta::assert_snapshot;
	use rstest::rstest;

	#[rstest]
	#[case("toml")]
	#[case("json")]
	#[case("jsonc")]
	#[case("json5")]
	#[case("yaml")]
	#[case("yml")]
	fn test_init_generate(#[case] format: &str) {
		let init = Init {
			force: false,
			format: format.to_string(),
		};
		let (filename, content) = init.generate().expect("Failed to generate");
		assert_eq!(filename, format!("biwa.{format}"));
		assert_snapshot!(format!("init_{}", format), content);
	}
}
