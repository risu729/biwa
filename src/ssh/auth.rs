use crate::Result;
use crate::config::types::{AuthMode, Config};
use crate::ssh::client::auth::Method;
use crate::ssh::target::ResolvedSshTarget;
use color_eyre::eyre::bail;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PublicKey, load_public_key, load_secret_key};
use std::env;
use std::io::{IsTerminal as _, stdin};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Standard private-key paths considered after configured and agent identities.
const DEFAULT_KEY_PATHS: &[&str] = &[".ssh/id_ed25519", ".ssh/id_rsa"];
/// Maximum number of unrelated agent identities offered automatically.
const UNRELATED_AGENT_LIMIT: usize = 10;

/// A deterministic list of concrete credentials to try.
#[derive(Debug)]
pub(super) struct AuthPlan {
	/// Credentials in attempt order. Each receives a fresh SSH transport.
	pub methods: Vec<Method>,
	/// Agent identities omitted by the unrelated-identity bound.
	pub skipped_agent_identities: usize,
}

/// Resolves credentials without opening a network connection or prompting.
pub(super) async fn resolve_auth(config: &Config, target: &ResolvedSshTarget) -> Result<AuthPlan> {
	let interaction_allowed = stdin().is_terminal();
	let password = env::var("BIWA_SSH_PASSWORD").ok();

	if config.ssh.auth == AuthMode::Password {
		return resolve_auth_with(
			config,
			target,
			password,
			Vec::new(),
			Vec::new(),
			interaction_allowed,
		);
	}

	let agent_keys = enumerate_agent_keys().await;
	let default_paths = default_key_paths();
	resolve_auth_with(
		config,
		target,
		password,
		agent_keys,
		default_paths,
		interaction_allowed,
	)
}

/// Resolves credentials from explicit local state for deterministic tests.
fn resolve_auth_with(
	config: &Config,
	target: &ResolvedSshTarget,
	password: Option<String>,
	agent_keys: Vec<PublicKey>,
	default_paths: Vec<PathBuf>,
	interaction_allowed: bool,
) -> Result<AuthPlan> {
	if config.ssh.auth == AuthMode::Password {
		return resolve_password_auth(config, target, password, interaction_allowed);
	}

	if password.is_some() {
		debug!("Ignoring BIWA_SSH_PASSWORD because public-key authentication is selected");
	}

	if let Some(path) = config.ssh.key_path.as_deref() {
		if !path.is_file() {
			bail!("Configured SSH key file not found: {}", path.display());
		}
		return Ok(AuthPlan {
			methods: vec![Method::with_key_file(path, interaction_allowed)],
			skipped_agent_identities: 0,
		});
	}

	let identity_paths = target
		.identity_files
		.iter()
		.map(|path| expand_identity_path(path))
		.collect::<Vec<_>>();
	let identity_keys = identity_paths
		.iter()
		.map(|path| public_key_for_identity(path).ok())
		.collect::<Vec<_>>();

	let mut methods = Vec::new();
	let mut selected_agent_keys = Vec::<PublicKey>::new();

	// Configured matches are always first and do not consume the unrelated-key allowance.
	for identity_key in identity_keys.iter().flatten() {
		for agent_key in &agent_keys {
			if same_key(identity_key, agent_key)
				&& !selected_agent_keys
					.iter()
					.any(|key| same_key(key, agent_key))
			{
				selected_agent_keys.push(agent_key.clone());
				methods.push(Method::with_agent_key(agent_key.clone()));
			}
		}
	}

	// A public IdentityFile may be only an agent selector; try a local file only when it can be a
	// private key. Loading and passphrase prompting remain lazy inside the concrete attempt.
	let mut selected_paths = Vec::<PathBuf>::new();
	for path in &identity_paths {
		if is_private_key_candidate(path) && !selected_paths.contains(path) {
			selected_paths.push(path.clone());
			methods.push(Method::with_key_file(path, interaction_allowed));
		}
	}

	let remaining_agent_keys = agent_keys
		.into_iter()
		.filter(|key| {
			!selected_agent_keys
				.iter()
				.any(|selected| same_key(selected, key))
		})
		.collect::<Vec<_>>();
	let skipped_agent_identities = remaining_agent_keys
		.len()
		.saturating_sub(UNRELATED_AGENT_LIMIT);
	for key in remaining_agent_keys.into_iter().take(UNRELATED_AGENT_LIMIT) {
		methods.push(Method::with_agent_key(key));
	}

	for path in default_paths {
		if path.is_file() && !selected_paths.contains(&path) {
			selected_paths.push(path.clone());
			methods.push(Method::with_key_file(path, interaction_allowed));
		}
	}

	if methods.is_empty() {
		bail!(
			"No SSH public key is available. Add a key to your SSH agent, create a standard key, configure ssh.key_path, or set ssh.auth = \"password\""
		);
	}

	info!(
		candidate_count = methods.len(),
		skipped_agent_identities, "Resolved SSH public-key candidates"
	);
	Ok(AuthPlan {
		methods,
		skipped_agent_identities,
	})
}

/// Resolves explicit password mode without inspecting keys or contacting the agent.
fn resolve_password_auth(
	config: &Config,
	target: &ResolvedSshTarget,
	password: Option<String>,
	interaction_allowed: bool,
) -> Result<AuthPlan> {
	if let Some(path) = config.ssh.key_path.as_deref() {
		bail!(
			"ssh.key_path `{}` cannot be used with ssh.auth = \"password\"; remove the key setting or select public-key authentication",
			path.display()
		);
	}
	let method = password.map_or_else(
		|| {
			Method::with_password_prompt(format!(
				"Password for {}@{}",
				target.user, target.hostname
			))
		},
		Method::with_password,
	);
	if matches!(method, Method::PasswordPrompt { .. }) && !interaction_allowed {
		bail!(
			"Password authentication was requested, but BIWA_SSH_PASSWORD is unset and interactive input is unavailable"
		);
	}
	Ok(AuthPlan {
		methods: vec![method],
		skipped_agent_identities: 0,
	})
}

/// Enumerates and normalizes identities from the agent selected by `SSH_AUTH_SOCK`.
async fn enumerate_agent_keys() -> Vec<PublicKey> {
	if env::var_os("SSH_AUTH_SOCK").is_none() {
		debug!("SSH_AUTH_SOCK is unset; skipping SSH agent");
		return Vec::new();
	}
	let Ok(mut agent) = AgentClient::connect_env().await else {
		debug!("Could not connect to the SSH agent; continuing with local keys");
		return Vec::new();
	};
	let Ok(identities) = agent.request_identities().await else {
		debug!("Could not enumerate SSH agent identities; continuing with local keys");
		return Vec::new();
	};

	let mut keys = Vec::<PublicKey>::new();
	for identity in identities {
		let public_key = identity.public_key().into_owned();
		if !keys.iter().any(|key| same_key(key, &public_key)) {
			keys.push(public_key);
		}
	}
	keys
}

/// Returns existing standard private-key paths in deterministic order.
fn default_key_paths() -> Vec<PathBuf> {
	let Some(home) = homedir::my_home().ok().flatten() else {
		return Vec::new();
	};
	DEFAULT_KEY_PATHS
		.iter()
		.map(|path| home.join(path))
		.collect()
}

/// Expands a leading home marker in an OpenSSH identity path.
fn expand_identity_path(path: &Path) -> PathBuf {
	path.strip_prefix("~").map_or_else(
		|_| path.to_path_buf(),
		|suffix| {
			homedir::my_home()
				.ok()
				.flatten()
				.map_or_else(|| path.to_path_buf(), |home| home.join(suffix))
		},
	)
}

/// Loads public key material from a selector, companion public key, or unencrypted private key.
fn public_key_for_identity(path: &Path) -> Result<PublicKey> {
	if let Ok(key) = load_public_key(path) {
		return Ok(key);
	}
	let companion = PathBuf::from(format!("{}.pub", path.to_string_lossy()));
	if let Ok(key) = load_public_key(&companion) {
		return Ok(key);
	}
	Ok(load_secret_key(path, None)?.public_key().clone())
}

/// Returns whether a path exists and is not itself a public-key selector.
fn is_private_key_candidate(path: &Path) -> bool {
	path.is_file() && load_public_key(path).is_err()
}

/// Compares public identity bytes while ignoring comments.
fn same_key(left: &PublicKey, right: &PublicKey) -> bool {
	left.key_data() == right.key_data()
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::{assert_eq, assert_matches};
	use std::fs;

	const KEY_ONE: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T";
	const KEY_TWO: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICQvZPc4//KJ9jQpzY7zfzt6isLmdQidqqeK4cdA0hY/";

	fn key(value: &str) -> PublicKey {
		PublicKey::from_openssh(value).expect("static key is valid")
	}

	fn target() -> ResolvedSshTarget {
		ResolvedSshTarget {
			lookup_host: "example.test".to_owned(),
			hostname: "example.test".to_owned(),
			port: 22,
			user: "alice".to_owned(),
			identity_files: Vec::new(),
		}
	}

	#[test]
	fn password_mode_uses_only_environment_password() -> Result<()> {
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;
		let plan = resolve_auth_with(
			&config,
			&target(),
			Some("secret".to_owned()),
			vec![key(KEY_ONE)],
			Vec::new(),
			false,
		)?;
		assert_matches!(plan.methods.as_slice(), [Method::Password(_)]);
		Ok(())
	}

	#[test]
	fn password_mode_rejects_key_path() {
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;
		config.ssh.key_path = Some(PathBuf::from("key"));
		let error = resolve_auth_with(
			&config,
			&target(),
			Some("secret".to_owned()),
			Vec::new(),
			Vec::new(),
			false,
		)
		.expect_err("key and password mode conflict");
		assert!(error.to_string().contains("cannot be used"));
	}

	#[test]
	fn identity_file_public_key_prioritizes_matching_agent() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let selector = dir.path().join("cse.pub");
		fs::write(&selector, KEY_TWO)?;
		let mut resolved = target();
		resolved.identity_files.push(selector);
		let plan = resolve_auth_with(
			&Config::default(),
			&resolved,
			None,
			vec![key(KEY_ONE), key(KEY_TWO)],
			Vec::new(),
			false,
		)?;
		let first = plan.methods.first().ok_or_else(|| {
			color_eyre::eyre::eyre!("matching agent key candidate was not created")
		})?;
		let Method::AgentKey { public_key } = first else {
			bail!("matching agent key must be first")
		};
		assert!(same_key(public_key, &key(KEY_TWO)));
		Ok(())
	}

	#[test]
	fn unrelated_agent_identities_are_bounded() -> Result<()> {
		let keys = (0..=UNRELATED_AGENT_LIMIT)
			.map(|index| {
				let mut value = key(KEY_ONE);
				value.set_comment(format!("{index}"));
				value
			})
			.collect::<Vec<_>>();
		let plan = resolve_auth_with(&Config::default(), &target(), None, keys, Vec::new(), false)?;
		assert_eq!(plan.methods.len(), UNRELATED_AGENT_LIMIT);
		assert_eq!(plan.skipped_agent_identities, 1);
		Ok(())
	}
}
