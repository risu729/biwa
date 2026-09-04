use crate::Result;
use crate::config::format::ConfigFormat;
use crate::config::types::{AuthMode, Config};
use crate::ssh::client::{Client, HostKeyVerificationFailed};
use crate::ssh::exec::connect;
use crate::ssh::target::ResolvedSshTarget;
use color_eyre::eyre::{Report, WrapErr as _, bail};
use console::style;
use dialoguer::{Confirm, Password};
use gethostname::gethostname;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::{Cipher, Kdf, LineEnding};
use russh::keys::{HashAlg, PrivateKey, PublicKey, load_public_key};
use std::fs;
use std::io::{IsTerminal as _, stdin};
use std::path::{Path, PathBuf, absolute};
use tracing::{debug, info};

/// Remote directory holding the user's SSH configuration.
const REMOTE_SSH_DIR: &str = "~/.ssh";
/// Remote file listing the authorized public keys.
const REMOTE_AUTHORIZED_KEYS: &str = "~/.ssh/authorized_keys";
/// Marker printed by the remote script when the key was appended.
const INSTALLED_MARKER: &str = "biwa-setup-ssh:installed";
/// Marker printed by the remote script when the key was already authorized.
const ALREADY_PRESENT_MARKER: &str = "biwa-setup-ssh:already-present";
/// Standard private-key paths considered when no key is selected explicitly.
const DEFAULT_KEY_PATHS: &[&str] = &[".ssh/id_ed25519", ".ssh/id_rsa"];
/// Path used for a generated key when no key is selected explicitly.
const GENERATED_KEY_PATH: &str = ".ssh/id_ed25519";
/// Salt length used when encrypting a generated private key, matching OpenSSH.
const BCRYPT_SALT_LEN: usize = 16;
/// bcrypt-pbkdf rounds used when encrypting a generated private key, matching OpenSSH.
const BCRYPT_ROUNDS: u32 = 16;

/// Set up SSH key authentication on the configured host.
#[derive(usage_rs::Args, Debug)]
#[usage(effect = "write")]
pub(super) struct SetupSsh {
	/// Private key to install. Defaults to `ssh.key_path` or a standard key path.
	#[usage(long)]
	key_path: Option<PathBuf>,

	/// Create a new key pair when the selected private key does not exist.
	#[usage(long, conflicts = "check")]
	generate: bool,

	/// Key algorithm used when generating a new key pair.
	#[usage(long, value_enum, default = "ed25519")]
	key_type: KeyType,

	/// Verify key authentication without changing anything.
	#[usage(long)]
	check: bool,

	/// Write the selected key into the nearest local biwa configuration file.
	#[usage(long, conflicts = "check")]
	write_config: bool,
}

/// Key algorithms biwa can generate locally.
#[derive(usage_rs::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum KeyType {
	/// Ed25519 key, the recommended algorithm.
	Ed25519,
}

/// Where the selected private key path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
	/// Selected by `--key-path`.
	CommandLine,
	/// Selected by `ssh.key_path`.
	Configuration,
	/// An existing key at a standard path.
	StandardPath,
	/// A standard path reserved for a key that does not exist yet.
	NewKey,
}

impl KeySource {
	/// Returns a short description used in progress messages.
	const fn describe(self) -> &'static str {
		match self {
			Self::CommandLine => "--key-path",
			Self::Configuration => "ssh.key_path",
			Self::StandardPath | Self::NewKey => "the standard key path",
		}
	}
}

/// Outcome of installing a public key into the remote `authorized_keys` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallOutcome {
	/// The key was appended to `authorized_keys`.
	Installed,
	/// The key was already listed in `authorized_keys`.
	AlreadyPresent,
}

impl SetupSsh {
	/// Runs the guided SSH key setup.
	pub(super) async fn run(self, quiet: bool) -> Result<()> {
		let config = Config::load()?;
		let target = ResolvedSshTarget::resolve(&config.ssh)?;
		let interactive = stdin().is_terminal();
		let (key_path, key_source) = select_key_path(
			self.key_path.as_deref().map(expand_key_path).transpose()?,
			config.ssh.key_path.clone(),
			&existing_default_key_paths(),
			generated_key_path(),
		)?;

		if self.check {
			return run_check(&config, &key_path, key_source, quiet).await;
		}

		let public_key = if key_path.is_file() {
			if try_key_authentication(&config, &key_path, quiet)
				.await?
				.is_none()
			{
				report_ready(&key_path, quiet);
				if self.write_config {
					write_config_key_path(&config, &key_path, quiet)?;
				}
				return Ok(());
			}
			load_key_pair(&key_path)?
		} else {
			self.create_key_pair(&key_path, &target, interactive, quiet)?
		};

		let authorized_key = authorized_key_line(&public_key)?;
		if !quiet {
			eprintln!(
				"Installing {} on {}@{}",
				style(key_fingerprint(&public_key)).bold(),
				target.user,
				target.hostname
			);
		}

		let client = connect_with_password(&config, quiet).await?;
		let outcome = install_authorized_key(&client, &authorized_key).await?;
		drop(client);
		if !quiet {
			match outcome {
				InstallOutcome::Installed => eprintln!(
					"{} Added the public key to {REMOTE_AUTHORIZED_KEYS}",
					style("✓").green().bold()
				),
				InstallOutcome::AlreadyPresent => eprintln!(
					"{} The public key was already in {REMOTE_AUTHORIZED_KEYS}",
					style("✓").green().bold()
				),
			}
		}

		if let Some(error) = try_key_authentication(&config, &key_path, quiet).await? {
			return Err(error).wrap_err(format!(
				"The public key was installed, but key authentication still failed. Check the server's `AuthorizedKeysFile` setting and the permissions of {REMOTE_SSH_DIR}"
			));
		}
		report_ready(&key_path, quiet);

		if self.write_config {
			write_config_key_path(&config, &key_path, quiet)?;
		} else {
			warn_about_password_mode(&config, &key_path, quiet);
		}

		Ok(())
	}

	/// Creates a key pair after confirming the choice when possible.
	fn create_key_pair(
		&self,
		key_path: &Path,
		target: &ResolvedSshTarget,
		interactive: bool,
		quiet: bool,
	) -> Result<PublicKey> {
		if !self.generate {
			if !interactive {
				bail!(
					"No SSH private key was found at {}. Pass --generate to create one, or select an existing key with --key-path",
					key_path.display()
				);
			}
			let confirmed = Confirm::new()
				.with_prompt(format!(
					"No SSH key found at {}. Generate a new one?",
					key_path.display()
				))
				.default(true)
				.interact()
				.map_err(|error| {
					Report::msg(format!("Failed to read the key generation answer: {error}"))
				})?;
			if !confirmed {
				bail!(
					"Aborted without generating a key. Select an existing key with --key-path to continue"
				);
			}
		}

		let comment = key_comment(&target.user);
		let public_key = generate_key_pair(key_path, self.key_type, &comment, interactive)?;
		if !quiet {
			eprintln!(
				"{} Generated {} at {}",
				style("✓").green().bold(),
				self.key_type.describe(),
				key_path.display()
			);
		}
		Ok(public_key)
	}
}

impl KeyType {
	/// Returns the human-readable algorithm name.
	const fn describe(self) -> &'static str {
		match self {
			Self::Ed25519 => "an Ed25519 key pair",
		}
	}
}

/// Verifies key authentication without changing local or remote state.
async fn run_check(
	config: &Config,
	key_path: &Path,
	key_source: KeySource,
	quiet: bool,
) -> Result<()> {
	if !key_path.is_file() {
		bail!(
			"No SSH private key exists at {} (selected by {}). Run `biwa setup-ssh --generate` to create one",
			key_path.display(),
			key_source.describe()
		);
	}

	let Some(error) = try_key_authentication(config, key_path, quiet).await? else {
		report_ready(key_path, quiet);
		return Ok(());
	};

	Err(error).wrap_err(format!(
		"Key authentication with {} is not working yet. Run `biwa setup-ssh` to install the matching public key",
		key_path.display()
	))
}

/// Reports that key authentication is ready.
fn report_ready(key_path: &Path, quiet: bool) {
	if !quiet {
		eprintln!(
			"{} Key authentication works with {}",
			style("✓").green().bold(),
			key_path.display()
		);
	}
}

/// Warns when configuration still forces password authentication.
fn warn_about_password_mode(config: &Config, key_path: &Path, quiet: bool) {
	if config.ssh.auth != AuthMode::Password || quiet {
		return;
	}
	eprintln!(
		"{} `ssh.auth` is still set to \"password\". Remove it, or run `biwa setup-ssh --write-config`, to use {}",
		style("!").yellow().bold(),
		key_path.display()
	);
}

/// Attempts key authentication with the selected key.
///
/// Returns `None` when the key authenticates, and the reason otherwise. A host-key
/// verification failure is returned as an error because retrying cannot help.
async fn try_key_authentication(
	config: &Config,
	key_path: &Path,
	quiet: bool,
) -> Result<Option<Report>> {
	let mut key_config = config.clone();
	key_config.ssh.auth = AuthMode::PublicKey;
	key_config.ssh.key_path = Some(key_path.to_path_buf());

	match connect(&key_config, quiet).await {
		Ok(client) => {
			drop(client);
			Ok(None)
		}
		Err(error) if error.downcast_ref::<HostKeyVerificationFailed>().is_some() => Err(error),
		Err(error) => {
			debug!(error = %error, key = %key_path.display(), "Key authentication is not available yet");
			Ok(Some(error))
		}
	}
}

/// Connects using password authentication only.
async fn connect_with_password(config: &Config, quiet: bool) -> Result<Client> {
	let mut password_config = config.clone();
	password_config.ssh.auth = AuthMode::Password;
	// Password mode rejects a configured key, which is expected during a migration.
	password_config.ssh.key_path = None;

	connect(&password_config, quiet)
		.await
		.wrap_err("Failed to connect with password authentication")
}

/// Appends the public key to the remote `authorized_keys` file when it is missing.
async fn install_authorized_key(client: &Client, authorized_key: &str) -> Result<InstallOutcome> {
	let script =
		install_authorized_key_script(authorized_key, &authorized_key_pattern(authorized_key)?);
	debug!(%script, "Installing the public key remotely");

	let result = client
		.execute(&script)
		.await
		.wrap_err("Failed to update the remote authorized_keys file")?;

	if result.exit_status != 0 {
		bail!(
			"Failed to update {REMOTE_AUTHORIZED_KEYS} (exit status {}): {}",
			result.exit_status,
			result.stderr.trim()
		);
	}

	if result.stdout.contains(INSTALLED_MARKER) {
		info!("Added the public key to the remote authorized_keys file");
		return Ok(InstallOutcome::Installed);
	}
	if result.stdout.contains(ALREADY_PRESENT_MARKER) {
		info!("The public key was already authorized");
		return Ok(InstallOutcome::AlreadyPresent);
	}

	bail!(
		"The remote authorized_keys update did not report a result. Remote output: {}",
		result.stdout.trim()
	)
}

/// Builds the idempotent remote script that authorizes one public key.
fn install_authorized_key_script(authorized_key: &str, key_pattern: &str) -> String {
	let quoted_key = shell_words::quote(authorized_key);
	let quoted_pattern = shell_words::quote(key_pattern);
	[
		"set -e".to_owned(),
		"umask 077".to_owned(),
		format!("mkdir -p {REMOTE_SSH_DIR}"),
		format!("chmod 700 {REMOTE_SSH_DIR}"),
		format!("touch {REMOTE_AUTHORIZED_KEYS}"),
		format!("chmod 600 {REMOTE_AUTHORIZED_KEYS}"),
		format!("if grep -q -F -e {quoted_pattern} {REMOTE_AUTHORIZED_KEYS}; then"),
		format!("echo {ALREADY_PRESENT_MARKER}"),
		"else".to_owned(),
		// A file whose last byte is not a newline would otherwise absorb the new entry.
		format!(
			"if [ -s {REMOTE_AUTHORIZED_KEYS} ] && [ -n \"$(tail -c 1 {REMOTE_AUTHORIZED_KEYS})\" ]; then"
		),
		format!("printf '\\n' >> {REMOTE_AUTHORIZED_KEYS}"),
		"fi".to_owned(),
		format!("printf '%s\\n' {quoted_key} >> {REMOTE_AUTHORIZED_KEYS}"),
		format!("echo {INSTALLED_MARKER}"),
		"fi".to_owned(),
	]
	.join("\n")
}

/// Renders a public key as one `authorized_keys` line.
fn authorized_key_line(public_key: &PublicKey) -> Result<String> {
	let line = public_key
		.to_openssh()
		.wrap_err("Failed to encode the public key")?;
	Ok(line.trim().to_owned())
}

/// Returns the algorithm and key material of an `authorized_keys` line.
///
/// The comment is excluded so a re-run recognizes a key installed under another comment.
fn authorized_key_pattern(authorized_key: &str) -> Result<String> {
	let mut fields = authorized_key.split_whitespace();
	let (Some(algorithm), Some(material)) = (fields.next(), fields.next()) else {
		bail!("Malformed public key line: expected an algorithm and key material")
	};
	Ok(format!("{algorithm} {material}"))
}

/// Returns the SHA-256 fingerprint of a public key.
fn key_fingerprint(public_key: &PublicKey) -> String {
	public_key.fingerprint(HashAlg::Sha256).to_string()
}

/// Selects the private key path to install.
fn select_key_path(
	cli_key_path: Option<PathBuf>,
	configured_key_path: Option<PathBuf>,
	existing_default_paths: &[PathBuf],
	new_key_path: Option<PathBuf>,
) -> Result<(PathBuf, KeySource)> {
	if let Some(path) = cli_key_path {
		return Ok((path, KeySource::CommandLine));
	}
	if let Some(path) = configured_key_path {
		return Ok((path, KeySource::Configuration));
	}
	if let Some(path) = existing_default_paths.first() {
		return Ok((path.clone(), KeySource::StandardPath));
	}
	new_key_path.map(|path| (path, KeySource::NewKey)).ok_or_else(|| {
		Report::msg(
			"Could not determine an SSH key path because the home directory is unknown. Select one with --key-path",
		)
	})
}

/// Returns the standard private-key paths that exist locally.
fn existing_default_key_paths() -> Vec<PathBuf> {
	let Some(home) = homedir::my_home().ok().flatten() else {
		return Vec::new();
	};
	DEFAULT_KEY_PATHS
		.iter()
		.map(|path| home.join(path))
		.filter(|path| path.is_file())
		.collect()
}

/// Returns the path used for a newly generated key.
fn generated_key_path() -> Option<PathBuf> {
	homedir::my_home()
		.ok()
		.flatten()
		.map(|home| home.join(GENERATED_KEY_PATH))
}

/// Expands a leading `~` and makes a selected key path absolute.
fn expand_key_path(path: &Path) -> Result<PathBuf> {
	let expanded = expand_home(path);
	absolute(&expanded)
		.wrap_err_with(|| format!("Failed to resolve the SSH key path {}", expanded.display()))
}

/// Expands a leading home marker in a local path.
fn expand_home(path: &Path) -> PathBuf {
	let Some(home) = homedir::my_home().ok().flatten() else {
		return path.to_path_buf();
	};
	let Some(text) = path.to_str() else {
		return path.to_path_buf();
	};
	if text == "~" {
		return home;
	}
	text.strip_prefix("~/")
		.map_or_else(|| path.to_path_buf(), |rest| home.join(rest))
}

/// Returns the companion public-key path for a private key.
fn public_key_path(private_key_path: &Path) -> PathBuf {
	PathBuf::from(format!("{}.pub", private_key_path.to_string_lossy()))
}

/// Loads the public key that pairs with an existing private key.
///
/// The private key itself is only parsed, never decrypted, so an encrypted key does not
/// require its passphrase here.
fn load_key_pair(private_key_path: &Path) -> Result<PublicKey> {
	let companion = public_key_path(private_key_path);
	if let Ok(public_key) = load_public_key(&companion) {
		debug!(path = %companion.display(), "Using the companion public key");
		return Ok(public_key);
	}

	let private_key = PrivateKey::read_openssh_file(private_key_path).wrap_err_with(|| {
		format!(
			"Failed to read the SSH private key {}. Create {} or select another key with --key-path",
			private_key_path.display(),
			companion.display()
		)
	})?;
	Ok(private_key.public_key().clone())
}

/// Returns the comment stored in a generated key.
fn key_comment(remote_user: &str) -> String {
	format!("{remote_user}@{}", gethostname().to_string_lossy())
}

/// Generates a key pair, writing the private key with owner-only permissions.
fn generate_key_pair(
	private_key_path: &Path,
	key_type: KeyType,
	comment: &str,
	interactive: bool,
) -> Result<PublicKey> {
	if private_key_path.exists() {
		bail!(
			"Refusing to overwrite the existing file {}",
			private_key_path.display()
		);
	}
	let public_key_path = public_key_path(private_key_path);
	if public_key_path.exists() {
		bail!(
			"Refusing to overwrite the existing file {}",
			public_key_path.display()
		);
	}
	create_key_directory(private_key_path)?;

	let mut private_key = match key_type {
		KeyType::Ed25519 => PrivateKey::from(Ed25519Keypair::from_seed(&random_bytes::<32>()?)),
	};
	private_key.set_comment(comment);
	let public_key = private_key.public_key().clone();

	let passphrase = if interactive {
		read_new_passphrase()?
	} else {
		String::new()
	};
	let stored_key = if passphrase.is_empty() {
		private_key
	} else {
		encrypt_private_key(&private_key, &passphrase)?
	};

	stored_key
		.write_openssh_file(private_key_path, LineEnding::LF)
		.wrap_err_with(|| {
			format!(
				"Failed to write the private key {}",
				private_key_path.display()
			)
		})?;
	public_key
		.write_openssh_file(&public_key_path)
		.wrap_err_with(|| {
			format!(
				"Failed to write the public key {}",
				public_key_path.display()
			)
		})?;

	Ok(public_key)
}

/// Creates the directory holding a generated key with owner-only permissions.
fn create_key_directory(private_key_path: &Path) -> Result<()> {
	let Some(parent) = private_key_path
		.parent()
		.filter(|path| !path.as_os_str().is_empty())
	else {
		return Ok(());
	};
	if parent.is_dir() {
		return Ok(());
	}

	fs::create_dir_all(parent)
		.wrap_err_with(|| format!("Failed to create the directory {}", parent.display()))?;
	#[cfg(unix)]
	{
		use std::fs::Permissions;
		use std::os::unix::fs::PermissionsExt as _;

		fs::set_permissions(parent, Permissions::from_mode(0o700))
			.wrap_err_with(|| format!("Failed to restrict permissions on {}", parent.display()))?;
	}
	Ok(())
}

/// Reads an optional passphrase for a newly generated key.
fn read_new_passphrase() -> Result<String> {
	Password::new()
		.with_prompt("Passphrase for the new key (empty for no passphrase)")
		.with_confirmation("Confirm passphrase", "The passphrases did not match")
		.allow_empty_password(true)
		.interact()
		.map_err(|error| Report::msg(format!("Failed to read the new passphrase: {error}")))
}

/// Encrypts a generated private key with the OpenSSH defaults.
fn encrypt_private_key(private_key: &PrivateKey, passphrase: &str) -> Result<PrivateKey> {
	let salt = random_bytes::<BCRYPT_SALT_LEN>()?;
	let checkint = random_bytes::<4>()?.iter().fold(0_u32, |value, byte| {
		value.wrapping_shl(8) | u32::from(*byte)
	});
	private_key
		.encrypt_with(
			Cipher::Aes256Ctr,
			Kdf::Bcrypt {
				salt: salt.to_vec(),
				rounds: BCRYPT_ROUNDS,
			},
			checkint,
			passphrase,
		)
		.wrap_err("Failed to encrypt the generated private key")
}

/// Returns cryptographically secure random bytes.
fn random_bytes<const N: usize>() -> Result<[u8; N]> {
	let mut bytes = [0_u8; N];
	getrandom::fill(&mut bytes).wrap_err("Failed to read random bytes for key generation")?;
	Ok(bytes)
}

/// Writes the selected key into the nearest local configuration file.
fn write_config_key_path(config: &Config, key_path: &Path, quiet: bool) -> Result<()> {
	let display_path = home_relative_path(key_path);
	let Some((config_path, format)) = Config::find_nearest_config_file()? else {
		report_manual_config(&display_path, "no biwa configuration file was found", quiet);
		return Ok(());
	};
	if format != ConfigFormat::Toml {
		report_manual_config(
			&display_path,
			&format!("{} is not a TOML configuration file", config_path.display()),
			quiet,
		);
		return Ok(());
	}

	let contents = fs::read_to_string(&config_path)
		.wrap_err_with(|| format!("Failed to read {}", config_path.display()))?;
	let mut updated = set_toml_ssh_value(&contents, "key_path", &display_path)?;
	if config.ssh.auth == AuthMode::Password {
		updated = set_toml_ssh_value(&updated, "auth", AuthMode::PublicKey.as_str())?;
	}

	if updated == contents {
		if !quiet {
			eprintln!(
				"{} {} already selects {display_path}",
				style("✓").green().bold(),
				config_path.display()
			);
		}
		return Ok(());
	}

	fs::write(&config_path, updated)
		.wrap_err_with(|| format!("Failed to write {}", config_path.display()))?;
	if !quiet {
		eprintln!(
			"{} Set ssh.key_path in {}",
			style("✓").green().bold(),
			config_path.display()
		);
	}
	Ok(())
}

/// Prints the configuration snippet to apply manually.
fn report_manual_config(key_path: &str, reason: &str, quiet: bool) {
	if quiet {
		return;
	}
	eprintln!(
		"{} Could not update the configuration automatically because {reason}. Add this manually:\n\n[ssh]\nkey_path = \"{key_path}\"\n",
		style("!").yellow().bold()
	);
}

/// Renders a path with the home directory replaced by `~`.
fn home_relative_path(path: &Path) -> String {
	homedir::my_home()
		.ok()
		.flatten()
		.and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf))
		.map_or_else(
			|| path.to_string_lossy().into_owned(),
			|relative| format!("~/{}", relative.to_string_lossy()),
		)
}

/// Sets one string value inside the `[ssh]` table of a TOML configuration file.
///
/// Only the affected line is rewritten so comments, ordering, and formatting survive.
fn set_toml_ssh_value(contents: &str, key: &str, value: &str) -> Result<String> {
	let encoded = toml::Value::String(value.to_owned()).to_string();
	let dotted_key = format!("ssh.{key}");
	let mut lines: Vec<String> = contents.lines().map(str::to_owned).collect();
	let mut current_table: Option<String> = None;
	let mut ssh_header_index: Option<usize> = None;
	let mut replaced = false;

	for (index, line) in lines.iter_mut().enumerate() {
		let trimmed = line.trim();
		if trimmed.starts_with('#') {
			continue;
		}
		if trimmed.starts_with('[') {
			current_table = Some(trimmed.to_owned());
			if trimmed == "[ssh]" {
				ssh_header_index = Some(index);
			}
			continue;
		}
		let Some(name) = assignment_key(trimmed) else {
			continue;
		};
		let matches = match current_table.as_deref() {
			Some("[ssh]") => name == key,
			None => name == dotted_key,
			Some(_) => false,
		};
		if matches {
			let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
			*line = format!("{indent}{name} = {encoded}");
			replaced = true;
			break;
		}
	}

	if !replaced {
		if let Some(index) = ssh_header_index {
			lines.insert(index.saturating_add(1), format!("{key} = {encoded}"));
		} else {
			if lines.last().is_some_and(|line| !line.trim().is_empty()) {
				lines.push(String::new());
			}
			lines.push("[ssh]".to_owned());
			lines.push(format!("{key} = {encoded}"));
		}
	}

	let mut updated = lines.join("\n");
	if contents.is_empty() || contents.ends_with('\n') {
		updated.push('\n');
	}
	verify_toml_ssh_value(&updated, key, value)?;
	Ok(updated)
}

/// Returns the key of a `key = value` assignment line.
fn assignment_key(line: &str) -> Option<&str> {
	let (key, _value) = line.split_once('=')?;
	let key = key.trim();
	if key.is_empty() || key.contains(char::is_whitespace) {
		return None;
	}
	Some(key)
}

/// Ensures the rewritten configuration parses and selects the intended value.
fn verify_toml_ssh_value(contents: &str, key: &str, value: &str) -> Result<()> {
	let parsed: toml::Value = toml::from_str(contents)
		.wrap_err("The updated configuration file is not valid TOML; no changes were written")?;
	let updated = parsed
		.get("ssh")
		.and_then(|ssh| ssh.get(key))
		.and_then(toml::Value::as_str);
	if updated == Some(value) {
		return Ok(());
	}
	bail!("Could not set ssh.{key} without rewriting unrelated configuration")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::{Cli, Commands};
	use crate::testing::write_test_ssh_private_key;
	use pretty_assertions::assert_eq;

	const PUBLIC_KEY: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T biwa";

	fn public_key() -> PublicKey {
		PublicKey::from_openssh(PUBLIC_KEY).expect("static key is valid")
	}

	/// Parses one `setup-ssh` invocation into its typed arguments.
	fn parse_setup_ssh<const N: usize>(args: [&str; N]) -> Result<SetupSsh> {
		let cli = Cli::try_parse_args(args)?;
		let Some(Commands::SetupSsh(command)) = cli.command else {
			bail!("the arguments must parse as the setup-ssh subcommand")
		};
		Ok(command)
	}

	#[test]
	fn setup_ssh_parses_without_arguments() -> Result<()> {
		let command = parse_setup_ssh(["biwa", "setup-ssh"])?;
		assert_eq!(command.key_path, None);
		assert!(!command.generate);
		assert!(!command.check);
		assert!(!command.write_config);
		assert_eq!(command.key_type, KeyType::Ed25519);
		Ok(())
	}

	#[test]
	fn setup_ssh_parses_every_flag() -> Result<()> {
		let command = parse_setup_ssh([
			"biwa",
			"setup-ssh",
			"--key-path",
			"/tmp/key",
			"--generate",
			"--key-type",
			"ed25519",
			"--write-config",
		])?;
		assert_eq!(command.key_path, Some(PathBuf::from("/tmp/key")));
		assert!(command.generate);
		assert!(command.write_config);
		assert_eq!(command.key_type, KeyType::Ed25519);
		Ok(())
	}

	#[test]
	fn setup_ssh_check_conflicts_with_writing_flags() -> Result<()> {
		assert!(parse_setup_ssh(["biwa", "setup-ssh", "--check"])?.check);

		let _generate = Cli::try_parse_args(["biwa", "setup-ssh", "--check", "--generate"])
			.expect_err("--check must not create a key");
		let _write_config = Cli::try_parse_args(["biwa", "setup-ssh", "--check", "--write-config"])
			.expect_err("--check must not write configuration");
		Ok(())
	}

	#[test]
	fn setup_ssh_rejects_unknown_key_types() {
		let _error = Cli::try_parse_args(["biwa", "setup-ssh", "--key-type", "rsa"])
			.expect_err("only supported algorithms parse");
	}

	#[test]
	fn authorized_key_line_drops_trailing_whitespace() -> Result<()> {
		assert_eq!(authorized_key_line(&public_key())?, PUBLIC_KEY);
		Ok(())
	}

	#[test]
	fn authorized_key_pattern_ignores_the_comment() -> Result<()> {
		assert_eq!(
			authorized_key_pattern(PUBLIC_KEY)?,
			"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T"
		);
		assert_eq!(
			authorized_key_pattern("ssh-ed25519 AAAA")?,
			"ssh-ed25519 AAAA"
		);
		let _error =
			authorized_key_pattern("ssh-ed25519").expect_err("a key line needs key material");
		Ok(())
	}

	#[test]
	fn install_script_quotes_the_key_and_is_idempotent() -> Result<()> {
		let script =
			install_authorized_key_script(PUBLIC_KEY, &authorized_key_pattern(PUBLIC_KEY)?);

		assert!(script.contains("mkdir -p ~/.ssh"));
		assert!(script.contains("chmod 700 ~/.ssh"));
		assert!(script.contains("chmod 600 ~/.ssh/authorized_keys"));
		assert!(script.contains("if grep -q -F -e "));
		assert!(script.contains(&format!("printf '%s\\n' '{PUBLIC_KEY}'")));
		assert!(script.contains(ALREADY_PRESENT_MARKER));
		assert!(script.contains(INSTALLED_MARKER));
		Ok(())
	}

	#[test]
	fn install_script_quotes_hostile_comments() -> Result<()> {
		let hostile = format!("{PUBLIC_KEY}; rm -rf ~");
		let script = install_authorized_key_script(&hostile, &authorized_key_pattern(&hostile)?);

		assert!(script.contains("'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T biwa; rm -rf ~'"));
		Ok(())
	}

	#[test]
	fn select_key_path_prefers_the_command_line() -> Result<()> {
		assert_eq!(
			select_key_path(
				Some(PathBuf::from("/cli")),
				Some(PathBuf::from("/config")),
				&[PathBuf::from("/default")],
				Some(PathBuf::from("/new")),
			)?,
			(PathBuf::from("/cli"), KeySource::CommandLine)
		);
		Ok(())
	}

	#[test]
	fn select_key_path_falls_back_through_every_source() -> Result<()> {
		assert_eq!(
			select_key_path(
				None,
				Some(PathBuf::from("/config")),
				&[PathBuf::from("/default")],
				Some(PathBuf::from("/new")),
			)?,
			(PathBuf::from("/config"), KeySource::Configuration)
		);
		assert_eq!(
			select_key_path(
				None,
				None,
				&[PathBuf::from("/default"), PathBuf::from("/other")],
				Some(PathBuf::from("/new")),
			)?,
			(PathBuf::from("/default"), KeySource::StandardPath)
		);
		assert_eq!(
			select_key_path(None, None, &[], Some(PathBuf::from("/new")))?,
			(PathBuf::from("/new"), KeySource::NewKey)
		);
		let _error = select_key_path(None, None, &[], None)
			.expect_err("an unknown home directory needs an explicit key path");
		Ok(())
	}

	#[test]
	fn public_key_path_appends_the_pub_suffix() {
		assert_eq!(
			public_key_path(Path::new("/home/user/.ssh/id_ed25519")),
			PathBuf::from("/home/user/.ssh/id_ed25519.pub")
		);
	}

	#[test]
	fn generated_key_pair_round_trips_without_a_passphrase() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let key_path = dir.path().join("nested/id_ed25519");

		let public_key = generate_key_pair(&key_path, KeyType::Ed25519, "biwa@test", false)?;

		assert_eq!(
			load_key_pair(&key_path)?.key_data(),
			public_key.key_data(),
			"the companion public key must match the private key"
		);
		assert_eq!(public_key.comment().as_str()?, "biwa@test");
		let stored = PrivateKey::read_openssh_file(&key_path)?;
		assert!(!stored.is_encrypted());
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt as _;

			assert_eq!(
				fs::metadata(&key_path)?.permissions().mode() & 0o777,
				0o600,
				"a generated private key must not be readable by other users"
			);
			assert_eq!(
				fs::metadata(dir.path().join("nested"))?
					.permissions()
					.mode() & 0o777,
				0o700
			);
		}
		Ok(())
	}

	#[test]
	fn generated_key_pair_never_overwrites_existing_files() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let key_path = dir.path().join("id_ed25519");
		write_test_ssh_private_key(&key_path)?;

		let error = generate_key_pair(&key_path, KeyType::Ed25519, "biwa@test", false)
			.expect_err("an existing private key must be preserved");
		assert!(error.to_string().contains("Refusing to overwrite"));

		fs::remove_file(&key_path)?;
		fs::write(public_key_path(&key_path), PUBLIC_KEY)?;
		let error = generate_key_pair(&key_path, KeyType::Ed25519, "biwa@test", false)
			.expect_err("an existing public key must be preserved");
		assert!(error.to_string().contains("Refusing to overwrite"));
		Ok(())
	}

	#[test]
	fn encrypted_generated_key_still_exposes_its_public_key() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let key_path = dir.path().join("id_ed25519");
		let private_key = PrivateKey::from(Ed25519Keypair::from_seed(&[7_u8; 32]));
		let encrypted = encrypt_private_key(&private_key, "passphrase")?;
		encrypted.write_openssh_file(&key_path, LineEnding::LF)?;

		assert!(PrivateKey::read_openssh_file(&key_path)?.is_encrypted());
		assert_eq!(
			load_key_pair(&key_path)?.key_data(),
			private_key.public_key().key_data()
		);
		Ok(())
	}

	#[test]
	fn toml_value_is_inserted_under_an_existing_ssh_table() -> Result<()> {
		let updated = set_toml_ssh_value(
			"#:schema https://biwa.takuk.me/schema/config.json\n\n[ssh]\nhost = \"cse\"\n",
			"key_path",
			"~/.ssh/id_ed25519",
		)?;

		assert_eq!(
			updated,
			"#:schema https://biwa.takuk.me/schema/config.json\n\n[ssh]\nkey_path = \"~/.ssh/id_ed25519\"\nhost = \"cse\"\n"
		);
		Ok(())
	}

	#[test]
	fn toml_value_replaces_an_existing_assignment() -> Result<()> {
		let updated = set_toml_ssh_value(
			"[ssh]\nhost = \"cse\"\n  key_path = \"~/.ssh/old\"\n\n[sync]\nauto = true\n",
			"key_path",
			"~/.ssh/id_ed25519",
		)?;

		assert_eq!(
			updated,
			"[ssh]\nhost = \"cse\"\n  key_path = \"~/.ssh/id_ed25519\"\n\n[sync]\nauto = true\n"
		);
		Ok(())
	}

	#[test]
	fn toml_value_replaces_a_dotted_assignment() -> Result<()> {
		let updated = set_toml_ssh_value(
			"ssh.host = \"cse\"\nssh.key_path = \"~/.ssh/old\"\n",
			"key_path",
			"~/.ssh/new",
		)?;

		assert_eq!(
			updated,
			"ssh.host = \"cse\"\nssh.key_path = \"~/.ssh/new\"\n"
		);
		Ok(())
	}

	#[test]
	fn toml_value_appends_a_missing_ssh_table() -> Result<()> {
		let updated = set_toml_ssh_value("[sync]\nauto = true\n", "key_path", "~/.ssh/id_ed25519")?;

		assert_eq!(
			updated,
			"[sync]\nauto = true\n\n[ssh]\nkey_path = \"~/.ssh/id_ed25519\"\n"
		);
		Ok(())
	}

	#[test]
	fn toml_value_ignores_assignments_in_other_tables() -> Result<()> {
		let updated = set_toml_ssh_value(
			"[sync]\nkey_path = \"unrelated\"\n\n[ssh]\nhost = \"cse\"\n",
			"key_path",
			"~/.ssh/id_ed25519",
		)?;

		assert!(updated.contains("[sync]\nkey_path = \"unrelated\""));
		assert!(updated.contains("[ssh]\nkey_path = \"~/.ssh/id_ed25519\""));
		Ok(())
	}

	#[test]
	fn toml_auth_mode_is_switched_to_public_key() -> Result<()> {
		let updated = set_toml_ssh_value(
			"[ssh]\nauth = \"password\"\n",
			"auth",
			AuthMode::PublicKey.as_str(),
		)?;

		assert_eq!(updated, "[ssh]\nauth = \"public-key\"\n");
		Ok(())
	}

	#[test]
	fn toml_rewrite_rejects_ambiguous_inline_tables() {
		let error = set_toml_ssh_value("ssh = { host = \"cse\" }\n", "key_path", "~/.ssh/key")
			.expect_err("an inline table cannot be extended by a new table header");
		assert!(error.to_string().contains("not valid TOML"));
	}

	#[test]
	fn assignment_key_reads_only_simple_assignments() {
		assert_eq!(assignment_key("key_path = \"value\""), Some("key_path"));
		assert_eq!(
			assignment_key("ssh.key_path=\"value\""),
			Some("ssh.key_path")
		);
		assert_eq!(assignment_key("no assignment here"), None);
		assert_eq!(assignment_key("two words = 1"), None);
		assert_eq!(assignment_key("= 1"), None);
	}

	#[test]
	fn home_relative_path_uses_a_tilde() {
		if let Some(home) = homedir::my_home().ok().flatten() {
			assert_eq!(
				home_relative_path(&home.join(".ssh/id_ed25519")),
				"~/.ssh/id_ed25519"
			);
		}
		assert_eq!(
			home_relative_path(Path::new("/opt/keys/id_ed25519")),
			"/opt/keys/id_ed25519"
		);
	}

	#[test]
	fn key_comment_names_the_remote_user_and_local_host() {
		let comment = key_comment("z5555555");
		assert!(comment.starts_with("z5555555@"));
	}

	#[test]
	fn key_source_descriptions_name_the_selecting_setting() {
		assert_eq!(KeySource::CommandLine.describe(), "--key-path");
		assert_eq!(KeySource::Configuration.describe(), "ssh.key_path");
		assert_eq!(KeySource::StandardPath.describe(), "the standard key path");
		assert_eq!(KeySource::NewKey.describe(), "the standard key path");
	}
}
