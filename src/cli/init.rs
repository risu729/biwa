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

		let mut content = Config::template(format);
		match format {
			ConfigFormat::Toml => {
				content = format!("#:schema {schema_url}\n\n{content}");
			}
			ConfigFormat::Yaml => {
				content = format!("# yaml-language-server: $schema={schema_url}\n{content}");
			}
			ConfigFormat::Json | ConfigFormat::Json5 => {
				content = content.lines().skip(1).collect::<Vec<_>>().join("\n");
				content = format!("{{\n  $schema: {schema_url}\n{content}");
			}
		}

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
