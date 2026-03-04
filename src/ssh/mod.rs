use tracing::info;

use crate::config::types::SshConfig;

/// Executes a command via SSH using the provided configuration.
pub async fn execute_command(
	config: &SshConfig,
	command: &str,
	args: &[String],
) -> eyre::Result<()> {
	info!(
		"Executing command: {} with args: {:?} on {}",
		command, args, config.host
	);
	unimplemented!("SSH command execution not implemented");
}
