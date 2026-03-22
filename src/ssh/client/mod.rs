/// Authentication types.
pub mod auth;
/// Command execution.
pub mod execute;

use self::auth::{Method, authenticate};
use self::execute::execute;

use crate::Result;
use alloc::sync::Arc;
use color_eyre::eyre::{Context as _, Report};
use core::fmt::Debug;
use core::result::Result as CoreResult;
use russh::Channel;
use russh::client::{Config, Handle, Handler, Msg, connect as russh_connect};
use russh::keys::PublicKey;
use tokio::net::ToSocketAddrs;
use tokio::net::lookup_host;

/// Handler for the SSH client.
struct ClientHandler;

impl Handler for ClientHandler {
	type Error = Report;

	async fn check_server_key(
		&mut self,
		_server_public_key: &PublicKey,
	) -> CoreResult<bool, Self::Error> {
		// TODO(#409): Implement proper host key verification
		tracing::debug!("skipping server key verification");
		Ok(true)
	}
}

/// An SSH client.
#[derive(Clone)]
pub struct Client {
	/// The active SSH connection handle.
	connection_handle: Arc<Handle<ClientHandler>>,
}

impl Client {
	/// Connect to a remote SSH server.
	pub async fn connect<T: ToSocketAddrs + Debug + Send + Sync>(
		addr: T,
		username: &str,
		auth: Method,
	) -> Result<Self> {
		let config = Arc::new(Config::default());

		let socket_addrs = lookup_host(&addr)
			.await
			.wrap_err("Failed to resolve addresses")?;

		let mut connect_res = None;
		let mut last_err: Option<Report> = None;

		for socket_addr in socket_addrs {
			let handler = ClientHandler;
			match russh_connect(Arc::clone(&config), socket_addr, handler).await {
				Ok(h) => {
					connect_res = Some(h);
					break;
				}
				Err(e) => {
					tracing::debug!(error = %e, %socket_addr, "Connection failed, trying next address");
					last_err = Some(e);
				}
			}
		}

		let Some(mut handle) = connect_res else {
			match last_err {
				Some(err) => {
					return Err(err.wrap_err(
						format!("Could not connect to any address for {addr:?}")
					));
				}
				None => {
					return Err(color_eyre::eyre::eyre!(
						"Could not connect: no addresses resolved for {addr:?}"
					));
				}
			}
		};

		let username = username.to_owned();

		authenticate(&mut handle, &username, auth).await?;

		Ok(Self {
			connection_handle: Arc::new(handle),
		})
	}

	/// Open a new SSH channel.
	pub async fn get_channel(&self) -> Result<Channel<Msg>> {
		self.connection_handle
			.channel_open_session()
			.await
			.wrap_err("Failed to open channel")
	}

	/// Execute a command and collect its stdout, stderr, and exit status.
	pub async fn execute(&self, command: &str) -> Result<execute::CommandExecutedResult> {
		let mut channel = self.get_channel().await?;
		execute(&mut channel, command).await
	}
}
