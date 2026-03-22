use crate::Result;
use color_eyre::eyre::{Context as _, bail};
use russh::client::Msg;
use russh::{Channel, ChannelMsg};

/// Result of a remote command execution.
pub struct CommandExecutedResult {
	/// Standard output.
	pub stdout: String,
	/// Standard error.
	pub stderr: String,
	/// Exit status code.
	pub exit_status: u32,
}

/// Execute a command and collect its stdout, stderr, and exit status.
pub(super) async fn execute(
	channel: &mut Channel<Msg>,
	command: &str,
) -> Result<CommandExecutedResult> {
	let mut stdout_buffer = vec![];
	let mut stderr_buffer = vec![];

	channel
		.exec(true, command)
		.await
		.wrap_err("Failed to exec command")?;

	let mut exit_status = None;

	while let Some(msg) = channel.wait().await {
		#[expect(
			clippy::wildcard_enum_match_arm,
			reason = "We only care about stdout, stderr, and exit status"
		)]
		match msg {
			ChannelMsg::Data { data } => {
				stdout_buffer.extend_from_slice(&data);
			}
			ChannelMsg::ExtendedData { data, ext } => {
				if ext == 1 {
					stderr_buffer.extend_from_slice(&data);
				}
			}
			ChannelMsg::ExitStatus {
				exit_status: status,
			} => {
				exit_status = Some(status);
			}
			_ => {}
		}
	}

	if let Some(status) = exit_status {
		Ok(CommandExecutedResult {
			stdout: String::from_utf8_lossy(&stdout_buffer).into_owned(),
			stderr: String::from_utf8_lossy(&stderr_buffer).into_owned(),
			exit_status: status,
		})
	} else {
		bail!("Command did not return exit status")
	}
}
