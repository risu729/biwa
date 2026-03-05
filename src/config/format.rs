/// Supported formats for the biwa config file.
#[expect(clippy::module_name_repetitions, reason = "No other way to name this")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
	/// TOML configuration format.
	Toml,
	/// YAML configuration format.
	Yaml,
	/// JSON configuration format.
	Json,
	/// JSON5 configuration format.
	Json5,
}

impl ConfigFormat {
	/// Returns a list of all supported formats.
	pub(super) const fn all() -> &'static [Self] {
		&[Self::Toml, Self::Yaml, Self::Json, Self::Json5]
	}

	/// Returns a list of extensions corresponding to the format.
	pub(super) const fn extensions(self) -> &'static [&'static str] {
		match self {
			Self::Toml => &["toml"],
			Self::Yaml => &["yaml", "yml"],
			Self::Json => &["json"],
			Self::Json5 => &["json5", "jsonc"],
		}
	}

	/// Returns the configuration format corresponding to the extension, if supported.
	pub fn from_extension(ext: &str) -> Option<Self> {
		let ext = ext.to_lowercase();
		Self::all()
			.iter()
			.find(|format| format.extensions().contains(&ext.as_str()))
			.copied()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn all_formats() {
		let all = ConfigFormat::all();
		assert_eq!(all.len(), 4);
		assert!(all.contains(&ConfigFormat::Toml));
		assert!(all.contains(&ConfigFormat::Yaml));
		assert!(all.contains(&ConfigFormat::Json));
		assert!(all.contains(&ConfigFormat::Json5));
	}

	#[test]
	fn extensions() {
		assert_eq!(ConfigFormat::Toml.extensions(), &["toml"]);
		assert_eq!(ConfigFormat::Yaml.extensions(), &["yaml", "yml"]);
		assert_eq!(ConfigFormat::Json.extensions(), &["json"]);
		assert_eq!(ConfigFormat::Json5.extensions(), &["json5", "jsonc"]);
	}

	#[test]
	fn from_extension() {
		assert_eq!(
			ConfigFormat::from_extension("toml"),
			Some(ConfigFormat::Toml)
		);
		assert_eq!(
			ConfigFormat::from_extension("TOML"),
			Some(ConfigFormat::Toml)
		);
		assert_eq!(
			ConfigFormat::from_extension("yaml"),
			Some(ConfigFormat::Yaml)
		);
		assert_eq!(
			ConfigFormat::from_extension("yml"),
			Some(ConfigFormat::Yaml)
		);
		assert_eq!(
			ConfigFormat::from_extension("YML"),
			Some(ConfigFormat::Yaml)
		);
		assert_eq!(
			ConfigFormat::from_extension("json"),
			Some(ConfigFormat::Json)
		);
		assert_eq!(
			ConfigFormat::from_extension("json5"),
			Some(ConfigFormat::Json5)
		);
		assert_eq!(
			ConfigFormat::from_extension("jsonc"),
			Some(ConfigFormat::Json5)
		);
		assert_eq!(ConfigFormat::from_extension("xml"), None);
		assert_eq!(ConfigFormat::from_extension("txt"), None);
	}
}
