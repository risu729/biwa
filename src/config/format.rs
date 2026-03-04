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
	pub const fn all() -> &'static [Self] {
		&[Self::Toml, Self::Yaml, Self::Json, Self::Json5]
	}

	/// Returns a list of extensions corresponding to the format.
	pub const fn extensions(self) -> &'static [&'static str] {
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
