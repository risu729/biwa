use figment::{
	Figment,
	providers::{Format, Json, Toml, Yaml},
};
use serde::de::DeserializeOwned;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
	Toml,
	Yaml,
	Json,
	Json5,
}

impl ConfigFormat {
	pub fn all() -> &'static [Self] {
		&[Self::Toml, Self::Yaml, Self::Json, Self::Json5]
	}

	pub fn extensions(&self) -> &'static [&'static str] {
		match self {
			Self::Toml => &["toml"],
			Self::Yaml => &["yaml", "yml"],
			Self::Json => &["json"],
			Self::Json5 => &["json5", "jsonc"],
		}
	}

	pub fn from_extension(ext: &str) -> Option<Self> {
		Self::all()
			.iter()
			.find(|format| format.extensions().contains(&ext))
			.copied()
	}
}

pub fn merge_config(figment: Figment, path: &Path, format: ConfigFormat) -> Figment {
	match format {
		ConfigFormat::Toml => figment.merge(Toml::file(path)),
		ConfigFormat::Yaml => figment.merge(Yaml::file(path)),
		ConfigFormat::Json => figment.merge(Json::file(path)),
		ConfigFormat::Json5 => figment.merge(Json5::file(path)),
	}
}

pub struct Json5;

impl Format for Json5 {
	type Error = figment::Error;
	const NAME: &'static str = "JSON5";

	fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, Self::Error> {
		json5::from_str(input)
			.map_err(|e| figment::Error::from(figment::error::Kind::Message(e.to_string())))
	}
}
