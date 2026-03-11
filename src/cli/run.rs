use crate::Result;
use crate::cli::sync::SyncArgs;
use crate::{
	config::types::Config,
	ssh::exec::execute_command,
	ssh::sync::{compute_project_remote_dir, sync_project},
};
use clap::Args;

/// Run a command on the CSE server.
#[derive(Args, Debug)]
#[clap(visible_alias = "r")]
pub(super) struct Run {
	/// Skip automatic synchronization before running the command (automatically set if --remote-dir is used).
	#[arg(long, overrides_with = "sync")]
	skip_sync: bool,

	/// Force automatic synchronization before running the command.
	#[arg(long, overrides_with = "skip_sync")]
	sync: bool,

	/// Synchronization options.
	#[clap(flatten)]
	sync_args: SyncArgs,

	/// The command to run.
	#[arg(required = true)]
	command: String,

	/// The arguments for the command.
	#[arg(allow_hyphen_values = true, trailing_var_arg = true)]
	command_args: Vec<String>,
}

/// Shared execution path for remote commands (used by both `biwa run` and implicit `biwa <args>`).
///
/// Resolves sync root and working directory, optionally syncs, then runs the command
/// in the resolved remote directory.
pub(super) async fn run_remote(
	config: &Config,
	sync_args: &SyncArgs,
	command: &str,
	command_args: &[String],
	should_sync: bool,
	quiet: bool,
	silent: bool,
) -> Result<()> {
	let sync_root = sync_args.resolve_sync_root(config)?;

	if should_sync {
		let options = sync_args.resolve_options();
		sync_project(
			config,
			&sync_root,
			&options,
			sync_args.remote_dir.as_deref(),
			quiet,
		)
		.await?;
	}

	// Determine working directory: explicit --remote-dir > computed synced dir
	let computed_working_dir;
	let working_dir: &str = if let Some(dir) = &sync_args.remote_dir {
		dir.as_str()
	} else {
		computed_working_dir = compute_project_remote_dir(config, &sync_root)?;
		&computed_working_dir
	};

	execute_command(
		config,
		command,
		command_args,
		Some(working_dir),
		quiet,
		silent,
	)
	.await?;
	Ok(())
}

impl Run {
	/// Determines whether synchronization should be performed before running the command.
	const fn should_sync(&self, config_sync_auto: bool) -> bool {
		if self.sync {
			true
		} else if self.skip_sync || self.sync_args.remote_dir.is_some() {
			false
		} else {
			config_sync_auto
		}
	}

	/// Run the execution logic for remote command.
	pub async fn run(self, config: &Config, quiet: bool, silent: bool) -> Result<()> {
		run_remote(
			config,
			&self.sync_args,
			&self.command,
			&self.command_args,
			self.should_sync(config.sync.auto),
			quiet,
			silent,
		)
		.await
	}
}

#[cfg(test)]
mod tests {
	use crate::cli::{Cli, Commands};
	use clap::Parser as _;
	use pretty_assertions::{assert_eq, assert_matches};

	#[test]
	fn run_command() {
		let args = Cli::parse_from(["biwa", "run", "ls", "-la"]);
		assert!(args.run_command_args.is_empty());
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(run.command, "ls");
			assert_eq!(run.command_args, vec!["-la"]);
		} else {
			assert_matches!(args.command, Some(Commands::Run(_)));
		}
	}

	#[test]
	fn run_command_alias() {
		let args = Cli::parse_from(["biwa", "r", "pwd"]);
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(run.command, "pwd");
			assert!(run.command_args.is_empty());
		} else {
			assert_matches!(args.command, Some(Commands::Run(_)));
		}
	}

	#[test]
	fn should_sync() {
		#[expect(
			clippy::unreachable,
			reason = "unreachable is acceptable in test helpers"
		)]
		let parse_run = |args: &[&str]| -> super::Run {
			let cli = Cli::parse_from(args);
			let Commands::Run(run) = cli.command.unwrap() else {
				unreachable!();
			};
			run
		};

		// Default (no flags) with config.sync.auto = true
		assert!(parse_run(&["biwa", "run", "ls"]).should_sync(true));
		// Default (no flags) with config.sync.auto = false
		assert!(!parse_run(&["biwa", "run", "ls"]).should_sync(false));

		// --skip-sync flag overrides config.sync.auto = true
		assert!(!parse_run(&["biwa", "run", "--skip-sync", "ls"]).should_sync(true));

		// --sync flag overrides config.sync.auto = false
		assert!(parse_run(&["biwa", "run", "--sync", "ls"]).should_sync(false));

		// --remote-dir implicitly disables sync by default
		assert!(!parse_run(&["biwa", "run", "-d", "/tmp", "ls"]).should_sync(true));

		// --sync overrides --remote-dir implicit disable
		assert!(parse_run(&["biwa", "run", "-d", "/tmp", "--sync", "ls"]).should_sync(false));

		// Test clap's overrides_with behavior: last flag wins
		assert!(!parse_run(&["biwa", "run", "--sync", "--skip-sync", "ls"]).should_sync(true));
		assert!(parse_run(&["biwa", "run", "--skip-sync", "--sync", "ls"]).should_sync(false));
	}
}
