use crate::Result;
use crate::cli::clean::spawn_background_cleanup;
use crate::cli::transfer::{TransferArgs, record_connection_use};
use crate::config::types::Config;
use crate::env_vars::parse_cli_env_vars;
use crate::{
	ssh::exec::{ExecuteCommandOptions, connect, execute_command_status},
	ssh::sync::{
		ensure_local_snapshot_unchanged, ensure_remote_matches_local_snapshot, pull_project,
		push_project, snapshot_local_project,
	},
};
use clap::Args;
use color_eyre::eyre::{Context as _, bail};
use tracing::warn;

/// Run a command on the CSE server.
#[derive(Args, Debug)]
#[clap(visible_alias = "r")]
#[expect(
	clippy::struct_excessive_bools,
	reason = "Clap represents the four independent transfer flags as booleans"
)]
pub(super) struct Run {
	/// Skip automatic synchronization before running the command (automatically selected if --remote-dir is used).
	#[arg(long, conflicts_with_all = ["sync", "pull", "pull_always"])]
	skip_sync: bool,

	/// Push project files before running, even when sync.auto is disabled or --remote-dir is set.
	#[arg(long, conflicts_with = "skip_sync")]
	sync: bool,

	/// Push before running, then mirror the remote project back after a successful command. May overwrite or delete local files.
	#[arg(long, conflicts_with_all = ["skip_sync", "pull_always"], verbatim_doc_comment)]
	pull: bool,

	/// Push before running, then mirror the remote project back after any confirmed exit status. May overwrite or delete local files.
	#[arg(long, conflicts_with_all = ["skip_sync", "pull"], verbatim_doc_comment)]
	pull_always: bool,

	/// Project transfer options.
	#[clap(flatten)]
	transfer_args: TransferArgs,

	/// Send environment variables to the remote process.
	/// Supports `NAME`, `NAME=value`, wildcard patterns like `NODE_*`, and exclusions like `!*PATH`.
	#[arg(long = "env")]
	env_vars: Vec<String>,

	/// The command to run.
	#[arg(required = true)]
	command: String,

	/// The arguments for the command.
	#[arg(allow_hyphen_values = true, trailing_var_arg = true)]
	command_args: Vec<String>,
}

/// File-transfer workflow surrounding a remote command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunTransferMode {
	/// Run without transferring project files.
	Skip,
	/// Push before running.
	Push,
	/// Push, run, then pull only after a successful command.
	PullOnSuccess,
	/// Push, run, then pull after any confirmed remote exit status.
	PullAlways,
}

impl RunTransferMode {
	/// Resolves the implicit-command transfer mode from configuration.
	pub(super) const fn from_auto(sync_auto: bool) -> Self {
		if sync_auto { Self::Push } else { Self::Skip }
	}

	/// Returns whether this workflow includes a pre-command push.
	const fn should_push(self) -> bool {
		!matches!(self, Self::Skip)
	}

	/// Returns whether this workflow includes a post-command pull.
	const fn should_pull(self, exit_status: u32) -> bool {
		match self {
			Self::PullOnSuccess => exit_status == 0,
			Self::PullAlways => true,
			Self::Skip | Self::Push => false,
		}
	}

	/// Returns whether this workflow is a round trip.
	const fn is_round_trip(self) -> bool {
		matches!(self, Self::PullOnSuccess | Self::PullAlways)
	}
}

/// Parsed remote command details shared across CLI entrypoints.
pub(super) struct RemoteCommand<'a> {
	/// Command name to execute remotely.
	pub command: &'a str,
	/// Command arguments to pass remotely.
	pub command_args: &'a [String],
	/// CLI `--env` arguments to merge with config env vars.
	pub cli_env_vars: &'a [String],
}

/// Shared execution path for remote commands (used by both `biwa run` and implicit `biwa <args>`).
///
/// Resolves sync root and working directory, optionally syncs, then runs the command
/// in the resolved remote directory.
pub(super) async fn run_remote(
	config: &Config,
	transfer_args: &TransferArgs,
	remote_command: RemoteCommand<'_>,
	transfer_mode: RunTransferMode,
	quiet: bool,
	silent: bool,
) -> Result<()> {
	let transfer = transfer_args.resolve(config)?;
	let client = connect(config, quiet || silent).await?;

	// Mark the directory as in use before remote work starts so background cleanup
	// does not treat an active old project as stale.
	record_connection_use(config, &transfer.remote_dir);

	let baseline_before_push = if transfer_mode.is_round_trip() {
		Some(snapshot_local_project(&transfer.local_root, config, &transfer.options).await?)
	} else {
		None
	};

	if transfer_mode.should_push() {
		push_project(
			&client,
			config,
			&transfer.local_root,
			&transfer.remote_dir,
			&transfer.options,
			quiet,
		)
		.await?;
	}
	let baseline = if let Some(before) = baseline_before_push {
		let after = snapshot_local_project(&transfer.local_root, config, &transfer.options).await?;
		ensure_local_snapshot_unchanged(&before, &after, "while the project was being pushed")?;
		ensure_remote_matches_local_snapshot(&client, config, &transfer.remote_dir, &after).await?;
		Some(after)
	} else {
		None
	};

	let cli_env_vars = parse_cli_env_vars(remote_command.cli_env_vars)?;
	let exit_status = execute_command_status(
		&client,
		config,
		ExecuteCommandOptions {
			command: remote_command.command,
			args: remote_command.command_args,
			cli_env_vars: &cli_env_vars,
			working_dir: Some(&transfer.remote_dir),
			quiet,
			silent,
		},
	)
	.await?;

	if transfer_mode.should_pull(exit_status) {
		pull_project(
			&client,
			config,
			&transfer.local_root,
			&transfer.remote_dir,
			&transfer.options,
			baseline.as_ref(),
			quiet,
		)
		.await
		.wrap_err_with(|| {
			if exit_status == 0 {
				format!(
					"Remote command succeeded, but pulling results from {} failed; remote changes remain available",
					transfer.remote_dir
				)
			} else {
				format!(
					"Remote command exited with code {exit_status}, and pulling results from {} also failed",
					transfer.remote_dir
				)
			}
		})?;
	}

	record_connection_use(config, &transfer.remote_dir);

	if exit_status != 0 {
		if transfer_mode == RunTransferMode::PullOnSuccess {
			bail!(
				"Remote command exited with code {exit_status}; results were not pulled. Run `biwa pull` to recover remote changes"
			);
		}
		bail!("Remote command exited with code {exit_status}");
	}

	// Spawn background cleanup daemon if enabled.
	if config.clean.auto
		&& let Err(e) = spawn_background_cleanup(config)
	{
		warn!(error = %e, "Failed to spawn background cleanup");
	}

	Ok(())
}

impl Run {
	/// Resolves the transfer workflow surrounding the remote command.
	const fn transfer_mode(&self, config_sync_auto: bool) -> RunTransferMode {
		if self.pull_always {
			RunTransferMode::PullAlways
		} else if self.pull {
			RunTransferMode::PullOnSuccess
		} else if self.sync {
			RunTransferMode::Push
		} else if self.skip_sync || self.transfer_args.remote_dir.is_some() {
			RunTransferMode::Skip
		} else {
			RunTransferMode::from_auto(config_sync_auto)
		}
	}

	/// Run the execution logic for remote command.
	pub async fn run(self, quiet: bool, silent: bool) -> Result<()> {
		let config = Config::load()?;
		run_remote(
			&config,
			&self.transfer_args,
			RemoteCommand {
				command: &self.command,
				command_args: &self.command_args,
				cli_env_vars: &self.env_vars,
			},
			self.transfer_mode(config.sync.auto),
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
			assert!(run.env_vars.is_empty());
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
			assert!(run.env_vars.is_empty());
		} else {
			assert_matches!(args.command, Some(Commands::Run(_)));
		}
	}

	#[test]
	fn run_supports_env_flag_forms() {
		let args = Cli::parse_from([
			"biwa", "run", "--env", "NODE_ENV", "--env", "DEBUG=1", "printenv",
		]);
		if let Some(Commands::Run(run)) = args.command {
			assert_eq!(
				run.env_vars,
				vec!["NODE_ENV".to_owned(), "DEBUG=1".to_owned()]
			);
		} else {
			assert_matches!(args.command, Some(Commands::Run(_)));
		}
	}

	#[test]
	fn transfer_mode() {
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

		assert_eq!(
			parse_run(&["biwa", "run", "ls"]).transfer_mode(true),
			super::RunTransferMode::Push
		);
		assert_eq!(
			parse_run(&["biwa", "run", "ls"]).transfer_mode(false),
			super::RunTransferMode::Skip
		);
		assert_eq!(
			parse_run(&["biwa", "run", "--skip-sync", "ls"]).transfer_mode(true),
			super::RunTransferMode::Skip
		);
		assert_eq!(
			parse_run(&["biwa", "run", "--sync", "ls"]).transfer_mode(false),
			super::RunTransferMode::Push
		);
		assert_eq!(
			parse_run(&["biwa", "run", "-d", "/tmp", "ls"]).transfer_mode(true),
			super::RunTransferMode::Skip
		);
		assert_eq!(
			parse_run(&["biwa", "run", "-d", "/tmp", "--sync", "ls"]).transfer_mode(false),
			super::RunTransferMode::Push
		);
		assert_eq!(
			parse_run(&["biwa", "run", "--pull", "ls"]).transfer_mode(false),
			super::RunTransferMode::PullOnSuccess
		);
		assert_eq!(
			parse_run(&["biwa", "run", "-d", "/tmp", "--pull-always", "ls"]).transfer_mode(false),
			super::RunTransferMode::PullAlways
		);
	}

	#[test]
	fn pull_modes_conflict_with_skip_sync() {
		for flags in [
			["--skip-sync", "--pull"],
			["--pull", "--skip-sync"],
			["--skip-sync", "--pull-always"],
			["--pull-always", "--skip-sync"],
		] {
			Cli::try_parse_from(["biwa", "run", flags[0], flags[1], "true"]).unwrap_err();
		}
	}

	#[test]
	fn transfer_flag_conflicts_and_redundancy() {
		for flags in [
			["--skip-sync", "--sync"],
			["--sync", "--skip-sync"],
			["--pull", "--pull-always"],
			["--pull-always", "--pull"],
		] {
			Cli::try_parse_from(["biwa", "run", flags[0], flags[1], "true"]).unwrap_err();
		}

		let run = parse_run_command(&["biwa", "run", "--sync", "--pull", "true"]);
		assert_eq!(
			run.transfer_mode(false),
			super::RunTransferMode::PullOnSuccess
		);
	}

	#[test]
	fn should_pull_matches_exit_policy() {
		use super::RunTransferMode::{PullAlways, PullOnSuccess, Push, Skip};

		assert!(!Skip.should_pull(0));
		assert!(!Push.should_pull(0));
		assert!(PullOnSuccess.should_pull(0));
		assert!(!PullOnSuccess.should_pull(7));
		assert!(PullAlways.should_pull(0));
		assert!(PullAlways.should_pull(7));
	}

	#[test]
	fn pull_after_separator_is_forwarded_to_remote_command() {
		let run = parse_run_command(&["biwa", "run", "--", "echo", "--pull"]);
		assert!(!run.pull);
		assert_eq!(run.command_args, vec!["--pull"]);
	}

	#[expect(
		clippy::unreachable,
		reason = "unreachable is acceptable in test helpers"
	)]
	fn parse_run_command(args: &[&str]) -> super::Run {
		let cli = Cli::parse_from(args);
		let Commands::Run(run) = cli.command.unwrap() else {
			unreachable!();
		};
		run
	}
}
