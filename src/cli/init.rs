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
				let config = Config::default();
				let mut value = serde_json::to_value(&config)?;
				if let Some(obj) = value.as_object_mut() {
					obj.insert("$schema".to_string(), json!(schema_url));
				}
				serde_json::to_string_pretty(&value)?
			}
			ConfigFormat::Json5 => {
				// Start from the JSON5 template (with comments)
				let template = Config::template(format);
				let body = template.lines().skip(1).collect::<Vec<_>>().join("\n");

				if self.format.eq_ignore_ascii_case("jsonc") {
					// For JSONC, keep comments but ensure keys are quoted and $schema uses JSON syntax.
					let body = quote_keys_for_jsonc(&body);
					format!("{{\n  \"$schema\": \"{schema_url}\",\n{body}")
				} else {
					// For JSON5, keep the original JSON5-style keys and schema.
					format!("{{\n  $schema: \"{schema_url}\",\n{body}")
				}
			}
		};

		Ok((filename, content))
	}
}

fn quote_keys_for_jsonc(body: &str) -> String {
	body
		.lines()
		.map(|line| {
			let trimmed = line.trim_start();
			if trimmed.is_empty() {
				return line.to_string();
			}

			let indent_len = line.len() - trimmed.len();
			let indent = &line[..indent_len];

			let (prefix, content) = if let Some(comment_body) = trimmed.strip_prefix("//") {
				let comment_trimmed = comment_body.trim_start();
				let prefix_len = comment_body.len() - comment_trimmed.len();
				(
					format!("//{}", &comment_body[..prefix_len]),
					comment_trimmed,
				)
			} else {
				(String::new(), trimmed)
			};

			if let Some(colon_idx) = content.find(':') {
				let (key, rest) = content.split_at(colon_idx);
				let is_simple_key = !key.starts_with('"')
					&& key
						.chars()
						.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
				if is_simple_key {
					return format!(r#"{indent}{prefix}"{key}"{rest}"#);
				}
			}

			line.to_string()
		})
		.collect::<Vec<_>>()
		.join("\n")
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
