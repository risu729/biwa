use tracing::info;

use crate::{Result, config::SshConfig};

pub async fn execute_command(config: &SshConfig, command: &str, args: &[String]) -> Result<()> {
	info!(
		"Executing command: {} with args: {:?} on {}",
		command, args, config.host
	);
	unimplemented!("SSH command execution not implemented");
}
