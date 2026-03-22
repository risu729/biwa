use crate::Result;
use crate::cache;
use crate::cli::clean::spawn_background_cleanup;
use crate::cli::sync::SyncArgs;
use crate::config::types::Config;
use crate::env_vars::parse_cli_env_vars;
use crate::{
	ssh::exec::{ExecuteCommandOptions, connect, execute_command},
	ssh::sync::{compute_project_remote_dir, sync_project},
};
use clap::Args;
use tracing::warn;

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
	sync_args: &SyncArgs,
	remote_command: RemoteCommand<'_>,
	should_sync: bool,
	quiet: bool,
	silent: bool,
) -> Result<()> {
	let sync_root = sync_args.resolve_sync_root(config)?;

	let client = connect(config, quiet || silent).await?;

	if should_sync {
		let options = sync_args.resolve_options();
		sync_project(
			&client,
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

	let cli_env_vars = parse_cli_env_vars(remote_command.cli_env_vars)?;
	execute_command(
		&client,
		config,
		ExecuteCommandOptions {
			command: remote_command.command,
			args: remote_command.command_args,
			cli_env_vars: &cli_env_vars,
			working_dir: Some(working_dir),
			quiet,
			silent,
		},
	)
	.await?;

	// Record the connection in the local cache.
	if let Err(e) = cache::record_connection(
		&config.ssh.host,
		&config.ssh.user,
		config.ssh.port,
		working_dir,
	) {
		warn!(error = %e, "Failed to record connection in cache");
	}

	// Spawn background cleanup daemon if enabled.
	if config.clean.auto
		&& let Err(e) = spawn_background_cleanup() {
			warn!(error = %e, "Failed to spawn background cleanup");
		}

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
	pub async fn run(self, quiet: bool, silent: bool) -> Result<()> {
		let config = Config::load()?;
		run_remote(
			&config,
			&self.sync_args,
			RemoteCommand {
				command: &self.command,
				command_args: &self.command_args,
				cli_env_vars: &self.env_vars,
			},
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
