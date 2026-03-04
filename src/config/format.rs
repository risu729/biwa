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

	pub fn extensions(self) -> &'static [&'static str] {
		match self {
			Self::Toml => &["toml"],
			Self::Yaml => &["yaml", "yml"],
			Self::Json => &["json"],
			Self::Json5 => &["json5", "jsonc"],
		}
	}

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
	fn test_all_formats() {
		let all = ConfigFormat::all();
		assert_eq!(all.len(), 4);
		assert!(all.contains(&ConfigFormat::Toml));
		assert!(all.contains(&ConfigFormat::Yaml));
		assert!(all.contains(&ConfigFormat::Json));
		assert!(all.contains(&ConfigFormat::Json5));
	}

	#[test]
	fn test_extensions() {
		assert_eq!(ConfigFormat::Toml.extensions(), &["toml"]);
		assert_eq!(ConfigFormat::Yaml.extensions(), &["yaml", "yml"]);
		assert_eq!(ConfigFormat::Json.extensions(), &["json"]);
		assert_eq!(ConfigFormat::Json5.extensions(), &["json5", "jsonc"]);
	}

	#[test]
	fn test_from_extension() {
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
