use crate::Result;
use crate::config::types::{AuthMode, Config};
use crate::ssh::client::auth::{AgentCredential, Method};
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
#[expect(clippy::redundant_pub_crate, reason = "Preferred by reviewer")]
pub(crate) struct AuthPlan {
	/// Credentials in attempt order. Each receives a fresh SSH transport.
	pub methods: Vec<Method>,
	/// Agent identities omitted by the unrelated-identity bound.
	pub skipped_agent_identities: usize,
}

/// Resolves credentials without opening a network connection or prompting.
#[expect(clippy::redundant_pub_crate, reason = "Preferred by reviewer")]
pub(crate) async fn resolve_auth(config: &Config, target: &ResolvedSshTarget) -> Result<AuthPlan> {
	let interaction_allowed = stdin().is_terminal();

	if config.ssh.auth == AuthMode::Password {
		return resolve_auth_with(
			config,
			target,
			env::var("BIWA_SSH_PASSWORD").ok(),
			Vec::new(),
			Vec::new(),
			interaction_allowed,
		);
	}
	if config.ssh.key_path.is_some() {
		return resolve_auth_with(
			config,
			target,
			None,
			Vec::new(),
			Vec::new(),
			interaction_allowed,
		);
	}

	let agent_credentials = enumerate_agent_credentials().await;
	let default_paths = default_key_paths();
	resolve_auth_with(
		config,
		target,
		None,
		agent_credentials,
		default_paths,
		interaction_allowed,
	)
}

/// Resolves credentials from explicit local state for deterministic tests.
fn resolve_auth_with(
	config: &Config,
	target: &ResolvedSshTarget,
	password: Option<String>,
	agent_credentials: Vec<AgentCredential>,
	default_paths: Vec<PathBuf>,
	interaction_allowed: bool,
) -> Result<AuthPlan> {
	if config.ssh.auth == AuthMode::Password {
		return resolve_password_auth(config, target, password, interaction_allowed);
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
	let mut selected_agent_credentials = Vec::<AgentCredential>::new();

	// Configured matches are always first and do not consume the unrelated-key allowance.
	for identity_key in identity_keys.iter().flatten() {
		for credential in &agent_credentials {
			if same_key(identity_key, credential.public_key())
				&& !selected_agent_credentials.contains(credential)
			{
				selected_agent_credentials.push(credential.clone());
				methods.push(Method::with_agent_credential(credential.clone()));
			}
		}
	}

	// A public IdentityFile may be only an agent selector; try a local file only when it can be a
	// private key. Loading and passphrase prompting remain lazy inside the concrete attempt.
	let mut selected_paths = Vec::<PathBuf>::new();
	for path in &identity_paths {
		if is_private_key_candidate(path) && !selected_paths.contains(path) {
			selected_paths.push(path.clone());
			methods.push(Method::with_automatic_key_file(path, interaction_allowed));
		}
	}

	let remaining_agent_credentials = agent_credentials
		.into_iter()
		.filter(|credential| !selected_agent_credentials.contains(credential))
		.collect::<Vec<_>>();
	let skipped_agent_identities = remaining_agent_credentials
		.len()
		.saturating_sub(UNRELATED_AGENT_LIMIT);
	for credential in remaining_agent_credentials
		.into_iter()
		.take(UNRELATED_AGENT_LIMIT)
	{
		methods.push(Method::with_agent_credential(credential));
	}

	for path in default_paths {
		if path.is_file() && !selected_paths.contains(&path) {
			selected_paths.push(path.clone());
			methods.push(Method::with_automatic_key_file(path, interaction_allowed));
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
async fn enumerate_agent_credentials() -> Vec<AgentCredential> {
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

	let mut credentials = Vec::<AgentCredential>::new();
	for identity in identities {
		let credential = AgentCredential::from_identity(identity);
		if !credentials.contains(&credential) {
			credentials.push(credential);
		}
	}
	credentials
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
	use crate::testing::{EnvCleanup, ssh_private_key_from_seed, write_test_ssh_private_key};
	use pretty_assertions::{assert_eq, assert_matches};
	use russh::keys::Certificate;
	use russh::keys::agent::AgentIdentity;
	use serial_test::serial;
	use std::fs;

	const KEY_ONE: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T";
	const KEY_TWO: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICQvZPc4//KJ9jQpzY7zfzt6isLmdQidqqeK4cdA0hY/";
	const CERTIFICATE: &str = "ssh-ed25519-cert-v01@openssh.com AAAAIHNzaC1lZDI1NTE5LWNlcnQtdjAxQG9wZW5zc2guY29tAAAAIBW/4zLqXWROWmN1sPgdySnH1GUsEFBjFrRwKKw71BoBAAAAIH1MFwI1oRdEifXgBQvWQfCBBtA/Pi8YCUE/I3wXFJo2AAAAAAAAAAAAAAABAAAAA2ZvbwAAAAAAAAAAAAAAAH//////////AAAAIwAAABFoZWxsb0BleGFtcGxlLmNvbQAAAAoAAAAGZm9vYmFyAAAAAAAAAAAAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIH1MFwI1oRdEifXgBQvWQfCBBtA/Pi8YCUE/I3wXFJo2AAAAUwAAAAtzc2gtZWQyNTUxOQAAAEDRoPdI48KyoaLgaDZsSGs80qBeYQOXBd84CX8GYzFt/L21rxF1EeuPOkgsx7Q39WllXp+FgMMojsHftK/DJHEN";

	fn key(value: &str) -> PublicKey {
		PublicKey::from_openssh(value).expect("static key is valid")
	}

	fn agent_key(value: &str) -> AgentCredential {
		AgentCredential::from_public_key(key(value))
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

	fn fixture_private_key(directory: &Path) -> Result<PathBuf> {
		let path = directory.join("id_ed25519");
		write_test_ssh_private_key(&path)?;
		Ok(path)
	}

	#[test]
	fn password_mode_uses_only_environment_password() -> Result<()> {
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;
		let plan = resolve_auth_with(
			&config,
			&target(),
			Some("secret".to_owned()),
			vec![agent_key(KEY_ONE)],
			Vec::new(),
			false,
		)?;
		assert_matches!(plan.methods.as_slice(), [Method::Password(_)]);
		Ok(())
	}

	#[test]
	fn password_mode_uses_prompt_when_interaction_is_available() -> Result<()> {
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;
		let plan = resolve_auth_with(&config, &target(), None, Vec::new(), Vec::new(), true)?;
		assert_matches!(
			plan.methods.as_slice(),
			[Method::PasswordPrompt { prompt }] if prompt == "Password for alice@example.test"
		);
		Ok(())
	}

	#[test]
	fn password_mode_requires_a_noninteractive_secret() {
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;
		let error = resolve_auth_with(&config, &target(), None, Vec::new(), Vec::new(), false)
			.expect_err("noninteractive password mode needs an environment secret");
		assert!(
			error
				.to_string()
				.contains("interactive input is unavailable")
		);
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
			vec![agent_key(KEY_ONE), agent_key(KEY_TWO)],
			Vec::new(),
			false,
		)?;
		let first = plan.methods.first().ok_or_else(|| {
			color_eyre::eyre::eyre!("matching agent key candidate was not created")
		})?;
		let Method::AgentKey { credential } = first else {
			bail!("matching agent key must be first")
		};
		assert!(same_key(credential.public_key(), &key(KEY_TWO)));
		Ok(())
	}

	#[test]
	fn identity_file_preserves_plain_and_certificate_agent_orderings() -> Result<()> {
		let certificate = Certificate::from_openssh(CERTIFICATE)?;
		let plain_credential =
			AgentCredential::from_public_key(certificate.public_key().clone().into());
		let certified = AgentCredential::from_identity(AgentIdentity::Certificate {
			certificate,
			comment: "certificate".to_owned(),
		});
		let dir = tempfile::tempdir()?;
		let selector = dir.path().join("selector.pub");
		fs::write(&selector, plain_credential.public_key().to_openssh()?)?;
		let mut resolved = target();
		resolved.identity_files.push(selector);

		for credentials in [
			vec![plain_credential.clone(), certified.clone()],
			vec![certified, plain_credential],
		] {
			let expected = credentials
				.iter()
				.cloned()
				.map(Method::with_agent_credential)
				.collect::<Vec<_>>();
			let auth_plan = resolve_auth_with(
				&Config::default(),
				&resolved,
				None,
				credentials,
				Vec::new(),
				false,
			)?;
			assert_eq!(auth_plan.methods, expected);
		}
		Ok(())
	}

	#[test]
	fn explicit_key_path_is_the_only_candidate() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let private_key = fixture_private_key(dir.path())?;
		let mut config = Config::default();
		config.ssh.key_path = Some(private_key.clone());
		let plan = resolve_auth_with(
			&config,
			&target(),
			Some("ignored".to_owned()),
			vec![agent_key(KEY_ONE)],
			vec![private_key.clone()],
			false,
		)?;
		assert_eq!(
			plan.methods,
			vec![Method::with_key_file(private_key, false)]
		);
		assert_eq!(plan.skipped_agent_identities, 0);
		Ok(())
	}

	#[test]
	fn explicit_key_path_must_exist() {
		let mut config = Config::default();
		config.ssh.key_path = Some(PathBuf::from("missing-key"));
		let error = resolve_auth_with(&config, &target(), None, Vec::new(), Vec::new(), false)
			.expect_err("missing configured key must fail early");
		assert!(error.to_string().contains("key file not found"));
	}

	#[test]
	fn local_identity_and_default_keys_are_ordered_and_deduplicated() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let private_key = fixture_private_key(dir.path())?;
		let mut resolved = target();
		resolved
			.identity_files
			.extend([private_key.clone(), private_key.clone()]);
		let extra_dir = tempfile::tempdir()?;
		let extra_key = extra_dir.path().join("extra-key");
		fs::copy(&private_key, &extra_key)?;

		let plan = resolve_auth_with(
			&Config::default(),
			&resolved,
			None,
			Vec::new(),
			vec![private_key.clone(), extra_key.clone()],
			false,
		)?;

		assert_eq!(
			plan.methods,
			vec![
				Method::with_automatic_key_file(private_key, false),
				Method::with_automatic_key_file(extra_key, false),
			]
		);
		Ok(())
	}

	#[test]
	fn public_key_selector_does_not_become_a_private_key_candidate() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let selector = dir.path().join("selector.pub");
		fs::write(&selector, KEY_ONE)?;
		let mut resolved = target();
		resolved.identity_files.push(selector);

		let error = resolve_auth_with(
			&Config::default(),
			&resolved,
			None,
			Vec::new(),
			Vec::new(),
			false,
		)
		.expect_err("a public selector without a matching agent key cannot authenticate");
		assert!(error.to_string().contains("No SSH public key is available"));
		Ok(())
	}

	#[test]
	fn companion_public_key_selects_an_agent_identity() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let identity = dir.path().join("agent-key");
		fs::write(identity.with_extension("pub"), KEY_TWO)?;
		let mut resolved = target();
		resolved.identity_files.push(identity);

		let plan = resolve_auth_with(
			&Config::default(),
			&resolved,
			None,
			vec![agent_key(KEY_TWO)],
			Vec::new(),
			false,
		)?;
		assert_matches!(plan.methods.as_slice(), [Method::AgentKey { .. }]);
		Ok(())
	}

	#[test]
	fn unrelated_agent_identities_are_bounded() -> Result<()> {
		let keys = (0..=UNRELATED_AGENT_LIMIT)
			.map(|index| {
				let seed = [u8::try_from(index + 1).expect("test index fits in u8"); 32];
				AgentCredential::from_public_key(
					ssh_private_key_from_seed(&seed).public_key().clone(),
				)
			})
			.collect::<Vec<_>>();
		let plan = resolve_auth_with(&Config::default(), &target(), None, keys, Vec::new(), false)?;
		assert_eq!(plan.methods.len(), UNRELATED_AGENT_LIMIT);
		assert_eq!(plan.skipped_agent_identities, 1);
		Ok(())
	}

	#[test]
	fn identity_helpers_cover_public_companion_private_and_missing_files() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let public = dir.path().join("direct.pub");
		fs::write(&public, KEY_ONE)?;
		assert!(same_key(&public_key_for_identity(&public)?, &key(KEY_ONE)));

		let companion_base = dir.path().join("companion");
		fs::write(companion_base.with_extension("pub"), KEY_TWO)?;
		assert!(same_key(
			&public_key_for_identity(&companion_base)?,
			&key(KEY_TWO)
		));

		let private = fixture_private_key(dir.path())?;
		let derived = public_key_for_identity(&private)?;
		let expected = load_public_key(
			Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh/id_ed25519.pub"),
		)?;
		assert!(same_key(&derived, &expected));
		assert!(is_private_key_candidate(&private));
		assert!(!is_private_key_candidate(&public));
		assert!(!is_private_key_candidate(&dir.path().join("missing")));
		let _error = public_key_for_identity(&dir.path().join("missing"))
			.expect_err("missing identity cannot be loaded");
		Ok(())
	}

	#[test]
	fn identity_path_expansion_preserves_absolute_paths_and_expands_home() {
		let absolute = Path::new("/tmp/id_ed25519");
		assert_eq!(expand_identity_path(absolute), absolute);

		if let Some(home) = homedir::my_home().ok().flatten() {
			assert_eq!(
				expand_identity_path(Path::new("~/.ssh/key")),
				home.join(".ssh/key")
			);
		}
	}

	#[test]
	fn default_key_paths_use_standard_home_locations() {
		let paths = default_key_paths();
		if let Some(home) = homedir::my_home().ok().flatten() {
			assert_eq!(
				paths,
				DEFAULT_KEY_PATHS
					.iter()
					.map(|path| home.join(path))
					.collect::<Vec<_>>()
			);
		} else {
			assert!(paths.is_empty());
		}
	}

	#[serial]
	#[tokio::test]
	async fn resolve_auth_uses_environment_password_in_password_mode() -> Result<()> {
		let _password = EnvCleanup::set("BIWA_SSH_PASSWORD", "secret");
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;

		let plan = resolve_auth(&config, &target()).await?;
		assert_eq!(plan.methods, vec![Method::with_password("secret")]);
		Ok(())
	}

	#[serial]
	#[tokio::test]
	async fn resolve_auth_continues_when_agent_socket_is_unavailable() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let missing_socket = dir.path().join("missing-agent.sock");
		let _socket = EnvCleanup::set("SSH_AUTH_SOCK", &missing_socket.to_string_lossy());
		let mut config = Config::default();
		config.ssh.key_path = Some(fixture_private_key(dir.path())?);

		let plan = resolve_auth(&config, &target()).await?;
		assert_matches!(plan.methods.as_slice(), [Method::PrivateKeyFile { .. }]);
		Ok(())
	}

	#[serial]
	#[tokio::test]
	async fn agent_enumeration_is_empty_without_a_socket() {
		let _socket = EnvCleanup::remove("SSH_AUTH_SOCK");
		assert!(enumerate_agent_credentials().await.is_empty());
	}

	#[serial]
	#[tokio::test]
	async fn agent_enumeration_continues_when_socket_is_unavailable() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let missing_socket = dir.path().join("missing-agent.sock");
		let _socket = EnvCleanup::set("SSH_AUTH_SOCK", &missing_socket.to_string_lossy());

		assert!(enumerate_agent_credentials().await.is_empty());
		Ok(())
	}
}
