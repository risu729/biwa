use crate::Result;
use color_eyre::eyre::{Context as _, bail};
use russh::client::Msg;
use russh::{Channel, ChannelMsg, Sig};

/// Result of a remote command execution.
pub struct CommandExecutedResult {
	/// Standard output.
	pub stdout: String,
	/// Standard error.
	pub stderr: String,
	/// Exit status code.
	pub exit_status: u32,
}

/// Maps a remote [`Sig`] to a synthetic non-zero exit code (POSIX-style `128 + signal`).
const fn exit_status_from_signal(sig: &Sig) -> u32 {
	match sig {
		Sig::HUP => 129,
		Sig::INT => 130,
		Sig::QUIT => 131,
		Sig::ILL => 132,
		Sig::ABRT => 134,
		Sig::FPE => 136,
		Sig::KILL => 137,
		Sig::USR1 => 138,
		Sig::SEGV => 139,
		Sig::ALRM => 142,
		Sig::PIPE => 141,
		Sig::TERM => 143,
		Sig::Custom(_) => 128,
	}
}

/// Waits for the SSH server to accept or reject a channel request.
pub async fn await_channel_confirmation(
	channel: &mut Channel<Msg>,
	request_name: &str,
) -> Result<()> {
	loop {
		match channel.wait().await {
			Some(ChannelMsg::Success) => {
				break;
			}
			Some(ChannelMsg::Failure) => {
				bail!("SSH server rejected {request_name}");
			}
			Some(_message) => {
				// Ignore unrelated channel messages and keep waiting for Success/Failure.
			}
			None => {
				bail!("SSH channel closed before {request_name} confirmation was received");
			}
		}
	}

	Ok(())
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

	await_channel_confirmation(channel, &format!("exec request for `{command}`")).await?;

	let mut exit_status = None;

	while let Some(msg) = channel.wait().await {
		collect_message(
			msg,
			(&mut stdout_buffer, &mut stderr_buffer, &mut exit_status),
		);
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

/// Collects output and exit status from a channel message.
fn collect_message(msg: ChannelMsg, output: (&mut Vec<u8>, &mut Vec<u8>, &mut Option<u32>)) {
	let (stdout_buffer, stderr_buffer, exit_status) = output;

	#[expect(
		clippy::wildcard_enum_match_arm,
		reason = "We only care about stdout, stderr, exit status, and exit signal"
	)]
	match msg {
		ChannelMsg::Data { data } => {
			stdout_buffer.extend_from_slice(&data);
		}
		ChannelMsg::ExtendedData { data, ext: 1 } => {
			stderr_buffer.extend_from_slice(&data);
		}
		ChannelMsg::ExitStatus {
			exit_status: status,
		} => {
			*exit_status = Some(status);
		}
		ChannelMsg::ExitSignal { signal_name, .. } => {
			*exit_status = Some(exit_status_from_signal(&signal_name));
		}
		_ => {}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use bytes::Bytes;
	use pretty_assertions::assert_eq;

	#[test]
	fn collect_message_routes_output_and_exit_status() {
		let mut stdout_buffer = Vec::new();
		let mut stderr_buffer = Vec::new();
		let mut exit_status = None;

		collect_message(
			ChannelMsg::Data {
				data: Bytes::from_static(b"out"),
			},
			(&mut stdout_buffer, &mut stderr_buffer, &mut exit_status),
		);
		collect_message(
			ChannelMsg::ExtendedData {
				data: Bytes::from_static(b"err"),
				ext: 1,
			},
			(&mut stdout_buffer, &mut stderr_buffer, &mut exit_status),
		);
		collect_message(
			ChannelMsg::ExtendedData {
				data: Bytes::from_static(b"ignored"),
				ext: 2,
			},
			(&mut stdout_buffer, &mut stderr_buffer, &mut exit_status),
		);
		collect_message(
			ChannelMsg::ExitStatus { exit_status: 7 },
			(&mut stdout_buffer, &mut stderr_buffer, &mut exit_status),
		);

		assert_eq!(stdout_buffer, b"out");
		assert_eq!(stderr_buffer, b"err");
		assert_eq!(exit_status, Some(7));

		collect_message(
			ChannelMsg::ExitSignal {
				signal_name: Sig::TERM,
				core_dumped: false,
				error_message: String::new(),
				lang_tag: String::new(),
			},
			(&mut stdout_buffer, &mut stderr_buffer, &mut exit_status),
		);

		assert_eq!(exit_status, Some(143));
	}
}
