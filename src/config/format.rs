#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
	Toml,
	Yaml,
	Json,
	Json5,
}

impl ConfigFormat {
	pub const fn all() -> &'static [Self] {
		&[Self::Toml, Self::Yaml, Self::Json, Self::Json5]
	}

	pub const fn extensions(self) -> &'static [&'static str] {
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
