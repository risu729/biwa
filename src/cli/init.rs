use crate::{Result, config::format::ConfigFormat, config::types::Config};
use clap::Args;
use eyre::bail;
use std::fs;
use std::path::Path;

/// Initialize a new configuration file
#[derive(Args, Debug)]
pub struct Init {
	/// Force overwrite if file exists
	#[arg(long, short)]
	force: bool,

	/// Format to generate (toml, json, json5, yaml)
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
		let config = Config::default();
		let schema_url = "https://biwa.takuk.me/schema/config.json";

		let format = ConfigFormat::from_extension(&self.format)
			.ok_or_else(|| eyre::eyre!("Unsupported format: {}", self.format))?;

		let content = match format {
			ConfigFormat::Toml => {
				let toml_str = toml::to_string_pretty(&config)?;
				format!("#:schema {schema_url}\n\n{toml_str}")
			}
			ConfigFormat::Json | ConfigFormat::Json5 => {
				let mut value = serde_json::to_value(&config)?;
				if let Some(obj) = value.as_object_mut() {
					obj.insert(
						"$schema".to_string(),
						serde_json::Value::String(schema_url.to_string()),
					);
				}
				if format == ConfigFormat::Json {
					serde_json::to_string_pretty(&value)?
				} else {
					json5::to_string(&value)?
				}
			}
			ConfigFormat::Yaml => {
				let value = serde_yaml::to_value(&config)?;
				let yaml_str = serde_yaml::to_string(&value)?;
				format!("# yaml-language-server: $schema={schema_url}\n{yaml_str}")
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
