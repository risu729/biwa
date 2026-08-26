use crate::Result;
use ::usage::Spec;
use ::usage::parse::{ParseOutput, ParseValue, TokenRole};
use std::sync::OnceLock;

/// Raw usage spec committed at the repository root.
///
/// This file is the source of truth for the CLI surface: commands, flags,
/// arguments, help text, and effects. It is embedded so the binary parses
/// arguments against the exact spec that completions and docs are built from.
const SPEC_KDL: &str = include_str!("../../biwa.usage.kdl");

/// Returns the parsed usage spec with the current crate version applied.
pub(super) fn usage_spec() -> &'static Spec {
	static SPEC: OnceLock<Spec> = OnceLock::new();
	SPEC.get_or_init(|| {
		let mut spec: Spec = SPEC_KDL
			.parse()
			.expect("embedded usage spec must be valid KDL");
		spec.version = Some(env!("CARGO_PKG_VERSION").to_owned());
		spec
	})
}

/// Generate a usage CLI spec.
///
/// See <https://usage.jdx.dev> for more information.
#[derive(Debug)]
pub(super) struct Usage;

impl Usage {
	/// Run the usage spec generation logic.
	#[expect(clippy::unused_self, reason = "usage subcommand doesn't have flags")]
	#[expect(
		clippy::unnecessary_wraps,
		reason = "usage subcommand doesn't return Err"
	)]
	pub(super) fn run(self) -> Result<()> {
		println!("{}", usage_spec().to_string().trim());
		Ok(())
	}
}

/// Returns the parsed value bound to a flag, looked up by its spec name.
fn flag_value_ref<'a>(output: &'a ParseOutput, name: &str) -> Option<&'a ParseValue> {
	output
		.flags
		.iter()
		.find_map(|(flag, value)| (flag.name == name).then_some(value))
}

/// Returns whether a boolean flag was given.
pub(super) fn flag_given(output: &ParseOutput, name: &str) -> bool {
	matches!(
		flag_value_ref(output, name),
		Some(&ParseValue::Bool(given)) if given
	)
}

/// Returns how many times a count flag was given.
pub(super) fn flag_count(output: &ParseOutput, name: &str) -> u8 {
	match flag_value_ref(output, name) {
		Some(ParseValue::MultiBool(occurrences)) => {
			u8::try_from(occurrences.len()).unwrap_or(u8::MAX)
		}
		_ => 0,
	}
}

/// Returns the value of a flag that takes a single argument.
pub(super) fn flag_value(output: &ParseOutput, name: &str) -> Option<String> {
	match flag_value_ref(output, name) {
		Some(ParseValue::String(value)) => Some(value.clone()),
		_ => None,
	}
}

/// Returns the values of a flag that can be given multiple times.
pub(super) fn flag_values(output: &ParseOutput, name: &str) -> Vec<String> {
	match flag_value_ref(output, name) {
		Some(ParseValue::MultiString(values)) => values.clone(),
		_ => Vec::new(),
	}
}

/// Returns the parsed value bound to a positional argument.
fn arg_value_ref<'a>(output: &'a ParseOutput, name: &str) -> Option<&'a ParseValue> {
	output
		.args
		.iter()
		.find_map(|(arg, value)| (arg.name == name).then_some(value))
}

/// Returns the value of a single-valued positional argument.
pub(super) fn arg_value(output: &ParseOutput, name: &str) -> Option<String> {
	match arg_value_ref(output, name) {
		Some(ParseValue::String(value)) => Some(value.clone()),
		_ => None,
	}
}

/// Returns the values of a variadic positional argument.
pub(super) fn arg_values(output: &ParseOutput, name: &str) -> Vec<String> {
	match arg_value_ref(output, name) {
		Some(ParseValue::MultiString(values)) => values.clone(),
		_ => Vec::new(),
	}
}

/// Returns the values of a trailing variadic argument, keeping a late `--` verbatim.
///
/// The usage parser always consumes the first explicit `--` as a separator, even
/// after a `double_dash=automatic` argument has started collecting values. Clap's
/// `trailing_var_arg` instead kept such a `--` as an ordinary value (for example
/// `biwa run sh -c 'test -d "$1"' -- <path>` forwards the `--` to the remote
/// shell). Re-insert the separator when the trailing capture had already begun.
pub(super) fn trailing_arg_values(output: &ParseOutput, name: &str) -> Vec<String> {
	let mut values = arg_values(output, name);
	let Some(separator_index) = output.tokens.iter().position(|token| {
		token
			.roles
			.iter()
			.any(|role| matches!(role, TokenRole::Separator))
	}) else {
		return values;
	};
	let values_before_separator: usize = output
		.tokens
		.iter()
		.take(separator_index)
		.flat_map(|token| &token.roles)
		.map(|role| {
			if let TokenRole::Arg {
				arg,
				values: bound_values,
			} = role
			{
				if arg.name == name {
					bound_values.len()
				} else {
					0
				}
			} else if let TokenRole::UnknownFlag {
				bound_as: Some(arg),
			} = role
			{
				usize::from(arg.name == name)
			} else {
				0
			}
		})
		.sum();
	if values_before_separator > 0 && values_before_separator <= values.len() {
		values.insert(values_before_separator, "--".to_owned());
	}
	values
}

#[cfg(test)]
mod tests {
	use super::*;
	use ::usage::SpecCommandEffect;
	use pretty_assertions::assert_eq;

	#[test]
	fn usage_spec_generation() {
		let spec = usage_spec();
		let output = spec.to_string();
		assert!(!output.is_empty());
		// Should contain the biwa command
		assert!(output.contains("biwa"));
	}

	#[test]
	fn usage_spec_matches_committed_artifact() {
		assert_eq!(
			usage_spec().to_string().trim(),
			include_str!("../../biwa.usage.kdl").trim()
		);
	}

	#[test]
	fn usage_spec_version_tracks_the_crate() {
		assert_eq!(
			usage_spec().version.as_deref(),
			Some(env!("CARGO_PKG_VERSION"))
		);
	}

	#[test]
	fn usage_spec_declares_effects() {
		use SpecCommandEffect::{Destructive, Read, Write};

		let spec = usage_spec();
		assert_eq!(spec.cmd.effect, None);
		assert_eq!(spec.cmd.max_effect(), Some(Destructive));

		let activate = spec
			.cmd
			.subcommands
			.get("activate")
			.expect("activate command should exist");
		assert_eq!(activate.effect, Some(Read));
		assert_eq!(activate.max_effect(), Some(Write));
		assert_eq!(
			activate
				.subcommands
				.get("doctor")
				.expect("doctor command should exist")
				.effect,
			Some(Read)
		);
		let install = activate
			.subcommands
			.get("install")
			.expect("install command should exist");
		assert_eq!(install.effect, Some(Write));
		assert_eq!(install.max_effect(), Some(Destructive));

		for command in ["run", "sync", "pull", "clean"] {
			assert_eq!(
				spec.cmd
					.subcommands
					.get(command)
					.expect("destructive command should exist")
					.effect,
				Some(Destructive)
			);
		}

		let init = spec
			.cmd
			.subcommands
			.get("init")
			.expect("init command should exist");
		assert_eq!(init.effect, Some(Write));
		assert_eq!(init.max_effect(), Some(Destructive));

		for command in ["schema", "completion", "usage"] {
			assert_eq!(
				spec.cmd
					.subcommands
					.get(command)
					.expect("read-only command should exist")
					.effect,
				Some(Read)
			);
		}
	}
}
