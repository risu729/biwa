use crate::Result;
use alloc::collections::BTreeMap;
use color_eyre::eyre::bail;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Strategy used to forward environment variables to the remote process.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EnvForwardMethod {
	/// Prefix the command with shell `export` statements.
	#[default]
	Export,
	/// Send environment variables through SSH `setenv` requests.
	Setenv,
}

/// Config representation for `env.vars`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum EnvVars {
	/// Array form such as `vars = ["NODE_ENV", "API_KEY=secret"]`.
	List(Vec<EnvVarItem>),
	/// Table form such as `[env.vars] NODE_ENV = true`.
	Table(BTreeMap<String, EnvVarConfigValue>),
}

impl Default for EnvVars {
	fn default() -> Self {
		Self::List(Vec::new())
	}
}

impl EnvVars {
	/// Returns the normalized env var specifications.
	pub fn specs(&self) -> Result<Vec<EnvVarSpec>> {
		match self {
			Self::List(items) => items.iter().map(EnvVarItem::to_spec_list).collect(),
			Self::Table(entries) => entries
				.iter()
				.map(|(name, value)| EnvVarSpec::from_config_value(name, value))
				.collect(),
		}
	}

	/// Builds a config value from normalized specs.
	#[must_use]
	pub fn from_specs(specs: Vec<EnvVarSpec>) -> Self {
		Self::List(
			specs
				.into_iter()
				.map(|spec| match spec.source {
					EnvVarSource::Inherit => EnvVarItem::String(spec.name),
					EnvVarSource::Value(value) => {
						EnvVarItem::String(format!("{}={value}", spec.name))
					}
				})
				.collect(),
		)
	}
}

/// An entry inside the array form of `env.vars`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum EnvVarItem {
	/// String form such as `NODE_ENV` or `NODE_ENV=production`.
	String(String),
	/// Inline table form such as `{ API_KEY = "secret" }`.
	Table(BTreeMap<String, EnvVarConfigValue>),
}

impl EnvVarItem {
	/// Converts one array item into a normalized environment variable specification.
	fn to_spec_list(&self) -> Result<EnvVarSpec> {
		match self {
			Self::String(value) => EnvVarSpec::from_inline_string(value),
			Self::Table(entries) => {
				let mut specs = entries
					.iter()
					.map(|(name, value)| EnvVarSpec::from_config_value(name, value))
					.collect::<Result<Vec<_>>>()?;

				if specs.len() != 1 {
					bail!(
						"Inline env.vars table entries must contain exactly one key (got {})",
						specs.len()
					);
				}

				Ok(specs.remove(0))
			}
		}
	}
}

/// Config value used in table and inline-table env var forms.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum EnvVarConfigValue {
	/// `true` means inherit the local value.
	Inherit(bool),
	/// String values are sent literally.
	Value(String),
}

/// Normalized environment variable specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVarSpec {
	/// Environment variable name.
	name: String,
	/// Source used to resolve the environment variable value.
	source: EnvVarSource,
}

impl EnvVarSpec {
	/// Creates an inheritance spec.
	#[must_use]
	pub fn inherit<T: Into<String>>(name: T) -> Self {
		Self {
			name: name.into(),
			source: EnvVarSource::Inherit,
		}
	}

	/// Creates a literal value spec.
	#[must_use]
	pub fn value<T: Into<String>, U: Into<String>>(name: T, value: U) -> Self {
		Self {
			name: name.into(),
			source: EnvVarSource::Value(value.into()),
		}
	}

	/// Parses a CLI-style string like `NAME` or `NAME=value`.
	fn from_inline_string(value: &str) -> Result<Self> {
		let trimmed = value.trim();
		if trimmed.is_empty() {
			bail!("Environment variable entries cannot be empty");
		}

		if let Some((name, raw_value)) = trimmed.split_once('=') {
			let name = name.trim();
			validate_env_var_name(name)?;
			Ok(Self::value(name, raw_value))
		} else {
			validate_env_var_name(trimmed)?;
			Ok(Self::inherit(trimmed))
		}
	}

	/// Builds a spec from table-based config input.
	fn from_config_value(name: &str, value: &EnvVarConfigValue) -> Result<Self> {
		validate_env_var_name(name)?;
		match value {
			EnvVarConfigValue::Inherit(true) => Ok(Self::inherit(name)),
			EnvVarConfigValue::Inherit(false) => {
				bail!("Invalid env.vars entry for {name}: only `true` is supported for inherit")
			}
			EnvVarConfigValue::Value(value) => Ok(Self::value(name, value)),
		}
	}

	/// Returns the environment variable name.
	#[must_use]
	pub fn name(&self) -> &str {
		&self.name
	}

	/// Returns the configured source.
	#[must_use]
	pub const fn source(&self) -> &EnvVarSource {
		&self.source
	}

	/// Returns whether this variable inherits the local value.
	#[must_use]
	pub const fn is_inherited(&self) -> bool {
		matches!(self.source, EnvVarSource::Inherit)
	}
}

/// Source of an environment variable value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvVarSource {
	/// Copy the value from the local process environment.
	Inherit,
	/// Use a literal value.
	Value(String),
}

/// Parses comma-separated env var specs like `NAME` or `NAME=value`.
pub fn parse_env_var_args(values: &[String], source: &str) -> Result<Vec<EnvVarSpec>> {
	let mut specs = Vec::new();
	for value in values {
		for token in value.split(',') {
			let token = token.trim();
			if token.is_empty() {
				bail!("{source} entries cannot be empty");
			}
			specs.push(EnvVarSpec::from_inline_string(token)?);
		}
	}
	Ok(specs)
}

/// Parses `--env` flag values.
pub fn parse_cli_env_vars(values: &[String]) -> Result<Vec<EnvVarSpec>> {
	parse_env_var_args(values, "`--env`")
}

/// Parses `BIWA_ENV_VARS` values.
pub fn parse_env_var_env(value: &str) -> Result<Vec<EnvVarSpec>> {
	parse_env_var_args(&[value.to_owned()], "BIWA_ENV_VARS")
}

/// Merges env var specs, with later entries overriding earlier ones.
#[must_use]
pub fn merge_env_vars(specs: Vec<EnvVarSpec>) -> Vec<EnvVarSpec> {
	let mut merged = Vec::new();
	for spec in specs {
		if let Some(index) = merged
			.iter()
			.position(|existing: &EnvVarSpec| existing.name == spec.name)
		{
			merged.remove(index);
		}
		merged.push(spec);
	}
	merged
}

/// Returns whether inheriting this variable is likely to be machine-specific.
#[must_use]
pub fn is_environment_dependent_env_var(name: &str) -> bool {
	let upper = name.to_ascii_uppercase();
	upper == "PATH"
		|| upper.ends_with("PATH")
		|| matches!(
			upper.as_str(),
			"HOME"
				| "PWD" | "OLDPWD"
				| "VIRTUAL_ENV"
				| "CONDA_PREFIX"
				| "PYTHONHOME"
				| "PYTHONPATH"
				| "NODE_PATH"
				| "NPM_CONFIG_PREFIX"
				| "JAVA_HOME"
				| "CLASSPATH"
				| "GOPATH" | "GOMODCACHE"
				| "GOBIN" | "GEM_HOME"
				| "GEM_PATH" | "BUNDLE_PATH"
				| "BUNDLE_BIN"
				| "PHP_INI_SCAN_DIR"
				| "CARGO_HOME"
				| "RUSTUP_HOME"
		)
}

/// Validates a POSIX-like environment variable name.
fn validate_env_var_name(name: &str) -> Result<()> {
	let mut chars = name.chars();
	let Some(first) = chars.next() else {
		bail!("Environment variable names cannot be empty");
	};

	if !(first == '_' || first.is_ascii_alphabetic()) {
		bail!("Invalid environment variable name: {name}");
	}

	if chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
		Ok(())
	} else {
		bail!("Invalid environment variable name: {name}")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use serde::Deserialize;

	#[derive(Deserialize)]
	struct EnvVarsWrapper {
		env: EnvWrapper,
	}

	#[derive(Deserialize)]
	struct EnvWrapper {
		vars: EnvVars,
	}

	#[test]
	fn env_vars_list_supports_inherit_and_literal_values() -> Result<()> {
		let vars = toml::from_str::<EnvVarsWrapper>(
			r#"[env]
vars = ["NODE_ENV", "API_KEY=secret", { DEBUG = "1" }]"#,
		)?
		.env
		.vars;

		assert_eq!(
			vars.specs()?,
			vec![
				EnvVarSpec::inherit("NODE_ENV"),
				EnvVarSpec::value("API_KEY", "secret"),
				EnvVarSpec::value("DEBUG", "1"),
			]
		);
		Ok(())
	}

	#[test]
	fn env_vars_table_supports_inherit_and_literal_values() -> Result<()> {
		let vars = toml::from_str::<EnvVarsWrapper>(
			r#"
			[env.vars]
			NODE_ENV = true
			API_KEY = "secret"
		"#,
		)?
		.env
		.vars;

		let specs = vars.specs()?;
		assert!(specs.contains(&EnvVarSpec::inherit("NODE_ENV")));
		assert!(specs.contains(&EnvVarSpec::value("API_KEY", "secret")));
		Ok(())
	}

	#[test]
	fn env_vars_false_is_rejected() -> Result<()> {
		let vars = toml::from_str::<EnvVarsWrapper>(
			"
			[env.vars]
			NODE_ENV = false
		",
		)?
		.env
		.vars;

		let error = vars.specs().unwrap_err().to_string();
		assert!(error.contains("only `true` is supported"));
		Ok(())
	}

	#[test]
	fn env_vars_array_of_tables_supports_multiple_entries() -> Result<()> {
		let vars = toml::from_str::<EnvVarsWrapper>(
			r#"[env]
vars = [{ NODE_ENV = "production" }, { API_KEY = "secret" }]"#,
		)?
		.env
		.vars;

		assert_eq!(
			vars.specs()?,
			vec![
				EnvVarSpec::value("NODE_ENV", "production"),
				EnvVarSpec::value("API_KEY", "secret"),
			]
		);
		Ok(())
	}

	#[test]
	fn parse_cli_env_values_supports_csv_and_repetition() -> Result<()> {
		let values = vec!["NODE_ENV,API_KEY".to_owned(), "DEBUG=1".to_owned()];
		assert_eq!(
			parse_cli_env_vars(&values)?,
			vec![
				EnvVarSpec::inherit("NODE_ENV"),
				EnvVarSpec::inherit("API_KEY"),
				EnvVarSpec::value("DEBUG", "1"),
			]
		);
		Ok(())
	}

	#[test]
	fn parse_env_var_env_supports_simple_names() -> Result<()> {
		assert_eq!(
			parse_env_var_env("NODE_ENV, API_KEY")?,
			vec![
				EnvVarSpec::inherit("NODE_ENV"),
				EnvVarSpec::inherit("API_KEY")
			]
		);
		Ok(())
	}

	#[test]
	fn parse_env_var_env_supports_values_and_inherit() -> Result<()> {
		assert_eq!(
			parse_env_var_env("NODE_ENV=prod,OTHER_ENV")?,
			vec![
				EnvVarSpec::value("NODE_ENV", "prod"),
				EnvVarSpec::inherit("OTHER_ENV"),
			]
		);
		Ok(())
	}

	#[test]
	fn merge_env_vars_prefers_later_entries() {
		assert_eq!(
			merge_env_vars(vec![
				EnvVarSpec::inherit("NODE_ENV"),
				EnvVarSpec::value("NODE_ENV", "production"),
			]),
			vec![EnvVarSpec::value("NODE_ENV", "production")]
		);
	}

	#[test]
	fn marks_environment_dependent_variables() {
		assert!(is_environment_dependent_env_var("PATH"));
		assert!(is_environment_dependent_env_var("LD_LIBRARY_PATH"));
		assert!(is_environment_dependent_env_var("HOME"));
		assert!(is_environment_dependent_env_var("PYTHONPATH"));
		assert!(is_environment_dependent_env_var("NODE_PATH"));
		assert!(is_environment_dependent_env_var("JAVA_HOME"));
		assert!(is_environment_dependent_env_var("GOPATH"));
		assert!(is_environment_dependent_env_var("GEM_HOME"));
		assert!(!is_environment_dependent_env_var("NODE_ENV"));
	}
}
