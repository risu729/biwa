use crate::Result;
use crate::cli::Cli;
use clap::{Args, CommandFactory as _};
use usage::{Spec, SpecCommand, SpecCommandEffect};

/// Generate a usage CLI spec.
///
/// See <https://usage.jdx.dev> for more information.
#[derive(Args, Debug)]
#[command(hide = true)]
pub(super) struct Usage;

impl Usage {
	/// Run the usage spec generation logic.
	#[expect(clippy::unused_self, reason = "usage subcommand doesn't have flags")]
	#[expect(
		clippy::unnecessary_wraps,
		reason = "usage subcommand doesn't return Err"
	)]
	pub(super) fn run(self) -> Result<()> {
		let spec = usage_spec();
		println!("{}", spec.to_string().trim());
		Ok(())
	}
}

/// Builds the usage specification and annotates command side effects.
fn usage_spec() -> Spec {
	let cli = Cli::command();
	let mut spec: Spec = cli.into();
	apply_effects(&mut spec);
	spec
}

/// Adds conservative effects to commands, flags, and arguments.
fn apply_effects(spec: &mut Spec) {
	use SpecCommandEffect::{Destructive, Read, Write};

	set_arg_effect(&mut spec.cmd, &[], "RUN_COMMAND_ARGS", Destructive);

	set_command_effect(&mut spec.cmd, &["activate"], Read);
	set_flag_effect(&mut spec.cmd, &["activate"], "shell", Write);
	set_command_effect(&mut spec.cmd, &["activate", "install"], Write);
	set_flag_effect(
		&mut spec.cmd,
		&["activate", "install"],
		"force",
		Destructive,
	);
	set_command_effect(&mut spec.cmd, &["activate", "doctor"], Read);

	for command in ["run", "sync", "pull", "clean"] {
		set_command_effect(&mut spec.cmd, &[command], Destructive);
	}

	set_command_effect(&mut spec.cmd, &["init"], Write);
	set_flag_effect(&mut spec.cmd, &["init"], "force", Destructive);

	for command in ["schema", "completion", "usage"] {
		set_command_effect(&mut spec.cmd, &[command], Read);
	}
}

/// Returns a mutable command at the provided path.
fn command_mut<'a>(root: &'a mut SpecCommand, path: &[&str]) -> &'a mut SpecCommand {
	let mut command = root;
	for name in path {
		command = command
			.subcommands
			.get_mut(*name)
			.expect("Clap-generated usage spec must contain annotated command");
	}
	command
}

/// Sets the effect for a command at the provided path.
fn set_command_effect(root: &mut SpecCommand, path: &[&str], effect: SpecCommandEffect) {
	command_mut(root, path).effect = Some(effect);
}

/// Sets the effect for a long flag on a command.
fn set_flag_effect(root: &mut SpecCommand, path: &[&str], long: &str, effect: SpecCommandEffect) {
	let command = command_mut(root, path);
	let flag = command
		.flags
		.iter_mut()
		.find(|flag| flag.long.iter().any(|candidate| candidate == long))
		.expect("Clap-generated usage spec must contain annotated flag");
	flag.effect = Some(effect);
}

/// Sets the effect for a positional argument on a command.
fn set_arg_effect(root: &mut SpecCommand, path: &[&str], name: &str, effect: SpecCommandEffect) {
	let command = command_mut(root, path);
	let arg = command
		.args
		.iter_mut()
		.find(|arg| arg.name == name)
		.expect("Clap-generated usage spec must contain annotated argument");
	arg.effect = Some(effect);
}

#[cfg(test)]
mod tests {
	use super::*;
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
		let spec = usage_spec();

		assert_eq!(
			spec.to_string().trim(),
			include_str!("../../biwa.usage.kdl").trim()
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
