use crate::Result;
use alloc::collections::{BTreeMap, BTreeSet};
use color_eyre::eyre::bail;
use globset::{Glob, GlobMatcher};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::{LazyLock, RwLock};
use tracing::warn;

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
	///
	/// Evaluated in the order written.
	List(Vec<EnvVarItem>),
	/// Table form such as `[env.vars] NODE_ENV = true`.
	///
	/// Evaluated in a deterministic order regardless of the table keys:
	/// 1. Inherit patterns (e.g., `"NODE_*" = true`)
	/// 2. Exact specifications (e.g., `"API_KEY" = "secret"`, `"NODE_ENV" = true`)
	/// 3. Exclusions (e.g., `"!*PATH" = true`)
	Table(BTreeMap<String, EnvVarConfigValue>),
}

impl Default for EnvVars {
	fn default() -> Self {
		Self::List(Vec::new())
	}
}

impl EnvVars {
	/// Returns the normalized env var rules.
	pub fn rules(&self) -> Result<Vec<EnvVarRule>> {
		match self {
			Self::List(items) => {
				let mut rules = Vec::new();
				for rule_batch in items.iter().map(EnvVarItem::to_rules) {
					rules.extend(rule_batch?);
				}
				Ok(rules)
			}
			Self::Table(entries) => {
				let mut rules: Vec<_> = entries
					.iter()
					.map(|(name, value)| EnvVarRule::from_config_entry(name, value))
					.collect::<Result<_>>()?;
				rules.sort();
				Ok(rules)
			}
		}
	}

	/// Builds a config value from normalized rules.
	#[must_use]
	pub fn from_rules(rules: Vec<EnvVarRule>) -> Self {
		Self::List(
			rules
				.into_iter()
				.map(|rule| match rule {
					EnvVarRule::Spec(spec) => match spec.source {
						EnvVarSource::Inherit => EnvVarItem::String(spec.name),
						EnvVarSource::Value(value) => {
							EnvVarItem::String(format!("{}={value}", spec.name))
						}
					},
					EnvVarRule::InheritPattern(pattern) => EnvVarItem::String(pattern),
					EnvVarRule::Exclude(selector) => {
						EnvVarItem::String(format!("!{}", selector.as_str()))
					}
				})
				.collect(),
		)
	}
}

/// Normalized environment variable rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvVarRule {
	/// Include inherited environment variables matching this wildcard pattern.
	InheritPattern(String),
	/// Exact environment variable specification.
	Spec(EnvVarSpec),
	/// Exclude already-selected environment variables matching this selector.
	Exclude(EnvVarSelector),
}

/// Selector used by exclusion rules.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvVarSelector {
	/// Exact environment variable name.
	Exact(String),
	/// Wildcard pattern using `*`.
	Pattern(String),
}

impl EnvVarSelector {
	/// Parses an env var selector from an inline string.
	fn parse(value: &str) -> Result<Self> {
		if value.is_empty() {
			bail!("Environment variable selectors cannot be empty");
		}

		if value.contains('*') {
			validate_env_var_pattern(value)?;
			Ok(Self::Pattern(value.to_owned()))
		} else {
			validate_env_var_name(value)?;
			Ok(Self::Exact(value.to_owned()))
		}
	}

	/// Returns whether the selector matches the environment variable name.
	#[must_use]
	pub fn matches(&self, name: &str) -> bool {
		match self {
			Self::Exact(exact) => exact == name,
			Self::Pattern(pattern) => wildcard_matches(pattern, name),
		}
	}

	/// Returns the selector as a string slice.
	#[must_use]
	pub fn as_str(&self) -> &str {
		match self {
			Self::Exact(value) | Self::Pattern(value) => value,
		}
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
	/// Converts one array item into a list of normalized environment variable rules.
	fn to_rules(&self) -> Result<Vec<EnvVarRule>> {
		match self {
			Self::String(value) => Ok(vec![EnvVarRule::from_inline_string(value)?]),
			Self::Table(entries) => {
				let mut rules: Vec<_> = entries
					.iter()
					.map(|(name, value)| EnvVarRule::from_config_entry(name, value))
					.collect::<Result<Vec<_>>>()?;
				rules.sort();
				Ok(rules)
			}
		}
	}
}

impl EnvVarRule {
	/// Parses a rule from a table `(name, value)` config entry.
	fn from_config_entry(name: &str, value: &EnvVarConfigValue) -> Result<Self> {
		let trimmed = name.trim();
		if trimmed.is_empty() {
			bail!("Environment variable entries cannot be empty");
		}

		if let Some(selector) = trimmed.strip_prefix('!') {
			let selector = EnvVarSelector::parse(selector.trim())?;
			match value {
				EnvVarConfigValue::Inherit(true) => Ok(Self::Exclude(selector)),
				EnvVarConfigValue::Inherit(false) | EnvVarConfigValue::Value(_) => bail!(
					"Invalid env.vars entry for {name}: only `true` is supported for exclude rules"
				),
			}
		} else {
			match EnvVarSelector::parse(trimmed)? {
				EnvVarSelector::Exact(exact_name) => match value {
					EnvVarConfigValue::Inherit(true) => {
						Ok(Self::Spec(EnvVarSpec::inherit(exact_name)))
					}
					EnvVarConfigValue::Inherit(false) => bail!(
						"Invalid env.vars entry for {name}: only `true` is supported for inherit rules"
					),
					EnvVarConfigValue::Value(val) => {
						Ok(Self::Spec(EnvVarSpec::value(exact_name, val)))
					}
				},
				EnvVarSelector::Pattern(pattern) => match value {
					EnvVarConfigValue::Inherit(true) => Ok(Self::InheritPattern(pattern)),
					EnvVarConfigValue::Inherit(false) | EnvVarConfigValue::Value(_) => bail!(
						"Invalid env.vars entry for {name}: only `true` is supported for pattern rules"
					),
				},
			}
		}
	}

	/// Parses a CLI-style string like `NAME`, `NAME=value`, `NODE_*`, or `!*PATH`.
	fn from_inline_string(value: &str) -> Result<Self> {
		let trimmed = value.trim();
		if trimmed.is_empty() {
			bail!("Environment variable entries cannot be empty");
		}

		if let Some((name, raw_value)) = trimmed.split_once('=') {
			let name = name.trim();
			validate_env_var_name(name)?;
			return Ok(Self::Spec(EnvVarSpec::value(name, raw_value)));
		}

		if let Some(selector) = trimmed.strip_prefix('!') {
			let selector = EnvVarSelector::parse(selector.trim())?;
			return Ok(Self::Exclude(selector));
		}

		match EnvVarSelector::parse(trimmed)? {
			EnvVarSelector::Exact(name) => Ok(Self::Spec(EnvVarSpec::inherit(name))),
			EnvVarSelector::Pattern(pattern) => Ok(Self::InheritPattern(pattern)),
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvVarSource {
	/// Copy the value from the local process environment.
	Inherit,
	/// Use a literal value.
	Value(String),
}

/// Parses comma-separated env var rules like `NAME`, `NAME=value`, `NODE_*`, or `!*PATH`.
pub fn parse_env_var_args(values: &[String], source: &str) -> Result<Vec<EnvVarRule>> {
	let mut rules = Vec::new();
	for value in values {
		for token in value.split(',') {
			let token = token.trim();
			if token.is_empty() {
				bail!("{source} entries cannot be empty");
			}
			rules.push(EnvVarRule::from_inline_string(token)?);
		}
	}
	Ok(rules)
}

/// Parses `--env` flag values.
pub fn parse_cli_env_vars(values: &[String]) -> Result<Vec<EnvVarRule>> {
	parse_env_var_args(values, "`--env`")
}

/// Parses `BIWA_ENV_VARS` values.
pub fn parse_env_var_env(value: &str) -> Result<Vec<EnvVarRule>> {
	parse_env_var_args(&[value.to_owned()], "BIWA_ENV_VARS")
}

/// Resolves ordered env var rules into exact environment variable specs.
#[must_use]
pub fn resolve_env_var_rules<I>(rules: I, available_names: &[String]) -> Vec<EnvVarSpec>
where
	I: IntoIterator<Item = EnvVarRule>,
{
	let mut resolved = BTreeMap::new();
	let mut names = available_names.to_vec();
	names.sort();

	for rule in rules {
		match rule {
			EnvVarRule::Spec(spec) => {
				resolved.insert(spec.name.clone(), spec);
			}
			EnvVarRule::InheritPattern(pattern) => {
				for name in &names {
					if wildcard_matches(&pattern, name) {
						if let Err(err) = validate_env_var_name(name) {
							warn!(
								name = %name,
								pattern = %pattern,
								"Skipping matched environment variable because its name is not POSIX-compliant: {err}"
							);
							continue;
						}
						resolved.insert(name.clone(), EnvVarSpec::inherit(name));
					}
				}
			}
			EnvVarRule::Exclude(selector) => {
				resolved.retain(|name, _| !selector.matches(name));
			}
		}
	}

	resolved.into_values().collect()
}

/// Returns sorted environment variable names from the local process.
#[must_use]
pub fn local_env_var_names() -> Vec<String> {
	let mut names = BTreeSet::new();
	for (name, _) in env::vars() {
		names.insert(name);
	}
	names.into_iter().collect()
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

/// Validates a wildcard env var pattern.
fn validate_env_var_pattern(pattern: &str) -> Result<()> {
	if pattern
		.chars()
		.all(|ch| ch == '*' || ch == '_' || ch.is_ascii_alphanumeric())
	{
		Ok(())
	} else {
		bail!("Invalid environment variable pattern: {pattern}")
	}
}

/// Global cache for compiled wildcard matchers.
static GLOB_CACHE: LazyLock<RwLock<BTreeMap<String, GlobMatcher>>> =
	LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Matches a wildcard pattern containing `*` against a candidate string.
///
/// Compiles each pattern into a `GlobMatcher` only once and reuses it for
/// subsequent calls, to avoid repeatedly compiling glob patterns in hot loops.
fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
	// Fast path: try to use an existing compiled matcher under a read lock.
	if let Some(matcher) = GLOB_CACHE
		.read()
		.expect("GLOB_CACHE read lock poisoned")
		.get(pattern)
	{
		return matcher.is_match(candidate);
	}

	// Slow path: compile a new matcher for this pattern.
	let matcher = Glob::new(pattern)
		.expect("environment variable wildcard patterns are validated before matching")
		.compile_matcher();

	// Insert the matcher into the cache under a write lock, but handle the case
	// where another thread may have inserted it in the meantime.
	GLOB_CACHE
		.write()
		.expect("GLOB_CACHE write lock poisoned")
		.entry(pattern.to_owned())
		.or_insert(matcher)
		.is_match(candidate)
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
			vars.rules()?,
			vec![
				EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV")),
				EnvVarRule::Spec(EnvVarSpec::value("API_KEY", "secret")),
				EnvVarRule::Spec(EnvVarSpec::value("DEBUG", "1")),
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

		let rules = vars.rules()?;
		assert!(rules.contains(&EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV"))));
		assert!(rules.contains(&EnvVarRule::Spec(EnvVarSpec::value("API_KEY", "secret"))));
		Ok(())
	}

	#[test]
	fn env_vars_table_supports_patterns_and_negation() -> Result<()> {
		let vars = toml::from_str::<EnvVarsWrapper>(
			r#"
			[env.vars]
			"NODE_*" = true
			"!*PATH" = true
			NODE_ENV = true
			API_KEY = "secret"
			"#,
		)?
		.env
		.vars;

		let rules = vars.rules()?;
		assert!(rules.contains(&EnvVarRule::InheritPattern("NODE_*".to_owned())));
		assert!(rules.contains(&EnvVarRule::Exclude(EnvVarSelector::Pattern(
			"*PATH".to_owned()
		))));
		assert!(rules.contains(&EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV"))));
		assert!(rules.contains(&EnvVarRule::Spec(EnvVarSpec::value("API_KEY", "secret"))));
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

		let error = vars.rules().unwrap_err().to_string();
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
			vars.rules()?,
			vec![
				EnvVarRule::Spec(EnvVarSpec::value("NODE_ENV", "production")),
				EnvVarRule::Spec(EnvVarSpec::value("API_KEY", "secret")),
			]
		);
		Ok(())
	}

	#[test]
	fn env_vars_list_supports_patterns_and_negation() -> Result<()> {
		let vars = toml::from_str::<EnvVarsWrapper>(
			r#"[env]
vars = ["NODE_*", "!*PATH", "NODE_ENV"]"#,
		)?
		.env
		.vars;

		assert_eq!(
			vars.rules()?,
			vec![
				EnvVarRule::InheritPattern("NODE_*".to_owned()),
				EnvVarRule::Exclude(EnvVarSelector::Pattern("*PATH".to_owned())),
				EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV")),
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
				EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV")),
				EnvVarRule::Spec(EnvVarSpec::inherit("API_KEY")),
				EnvVarRule::Spec(EnvVarSpec::value("DEBUG", "1")),
			]
		);
		Ok(())
	}

	#[test]
	fn parse_env_var_env_supports_simple_names() -> Result<()> {
		assert_eq!(
			parse_env_var_env("NODE_ENV, API_KEY")?,
			vec![
				EnvVarRule::Spec(EnvVarSpec::inherit("NODE_ENV")),
				EnvVarRule::Spec(EnvVarSpec::inherit("API_KEY"))
			]
		);
		Ok(())
	}

	#[test]
	fn parse_env_var_env_supports_values_and_inherit() -> Result<()> {
		assert_eq!(
			parse_env_var_env("NODE_ENV=prod,OTHER_ENV")?,
			vec![
				EnvVarRule::Spec(EnvVarSpec::value("NODE_ENV", "prod")),
				EnvVarRule::Spec(EnvVarSpec::inherit("OTHER_ENV")),
			]
		);
		Ok(())
	}

	#[test]
	fn parse_env_var_env_supports_patterns_and_negation() -> Result<()> {
		assert_eq!(
			parse_env_var_env("NODE_*,!*PATH")?,
			vec![
				EnvVarRule::InheritPattern("NODE_*".to_owned()),
				EnvVarRule::Exclude(EnvVarSelector::Pattern("*PATH".to_owned())),
			]
		);
		Ok(())
	}

	#[test]
	fn wildcard_matches_supports_prefix_suffix_and_contains_patterns() {
		assert!(wildcard_matches("NODE_*", "NODE_ENV"));
		assert!(wildcard_matches("*PATH", "LD_LIBRARY_PATH"));
		assert!(wildcard_matches("PY*PATH", "PYTHONPATH"));
		assert!(!wildcard_matches("NODE_*", "PATH"));
	}

	#[test]
	fn resolve_env_var_rules_supports_patterns_negation_and_override_order() {
		let available = vec![
			"APP_PATH".to_owned(),
			"NODE_ENV".to_owned(),
			"NODE_PATH".to_owned(),
			"RUST_LOG".to_owned(),
		];

		assert_eq!(
			resolve_env_var_rules(
				vec![
					EnvVarRule::InheritPattern("NODE_*".to_owned()),
					EnvVarRule::Exclude(EnvVarSelector::Pattern("*PATH".to_owned())),
					EnvVarRule::Spec(EnvVarSpec::value("NODE_ENV", "production")),
				],
				&available,
			),
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
