use crate::Result;
use crate::cli::Cli;

/// Generate usage command specifications.
///
/// See <https://usage.jdx.dev> for more information.
#[derive(usage_rs::Args, Debug)]
#[usage(hide = true, effect = "read")]
pub(super) struct Usage;

impl Usage {
	/// Run the usage spec generation logic.
	#[expect(clippy::unused_self, reason = "usage subcommand doesn't have flags")]
	#[expect(
		clippy::unnecessary_wraps,
		reason = "usage subcommand doesn't return Err"
	)]
	pub(super) fn run(self) -> Result<()> {
		println!("{}", Cli::to_kdl().trim());
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use ::usage::{Spec, SpecCommandEffect};
	use pretty_assertions::assert_eq;

	/// Parses the emitted spec the way external usage consumers do.
	fn parsed_spec() -> Spec {
		Cli::to_kdl()
			.parse()
			.expect("emitted usage spec must be valid KDL")
	}

	#[test]
	fn usage_spec_generation() {
		let output = Cli::to_kdl();
		assert!(!output.is_empty());
		// Should contain the biwa command
		assert!(output.contains("biwa"));
	}

	#[test]
	fn usage_spec_matches_committed_artifact() {
		assert_eq!(
			Cli::to_kdl().trim(),
			include_str!("../../biwa.usage.kdl").trim()
		);
	}

	#[test]
	fn usage_spec_version_tracks_the_crate() {
		assert_eq!(
			parsed_spec().version.as_deref(),
			Some(env!("CARGO_PKG_VERSION"))
		);
	}

	#[test]
	fn usage_spec_declares_effects() {
		use SpecCommandEffect::{Destructive, Read, Write};

		let spec = parsed_spec();
		// usage-rs allows effects on subcommands and flags only — the derive
		// rejects one on the root command, and the spec format has nowhere to
		// put one on the hidden implicit-run argument — so the implicit
		// `biwa <cmd>` form (which runs arbitrary remote commands) is left
		// effect-unknown for consumers. Spec consumers must not treat the
		// missing effect as safe.
		assert_eq!(spec.cmd.effect, None);
		assert_eq!(spec.cmd.max_effect(), None);

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
