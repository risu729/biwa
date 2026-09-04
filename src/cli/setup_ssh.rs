use crate::Result;
use crate::config::format::ConfigFormat;
use crate::config::types::{AuthMode, Config};
use crate::ssh::auth::resolve_auth;
use crate::ssh::client::auth::{AuthenticationFailed, AuthenticationFailureKind};
use crate::ssh::client::{Client, HostKeyVerificationFailed};
use crate::ssh::exec::connect;
use crate::ssh::target::ResolvedSshTarget;
use color_eyre::eyre::{Report, WrapErr as _, bail};
use console::style;
use dialoguer::{Confirm, Password};
use gethostname::gethostname;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::{Cipher, Kdf, LineEnding};
use russh::keys::{HashAlg, PrivateKey, PublicKey};
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal as _, Write as _, stdin};
use std::path::{Path, PathBuf, absolute};
use tracing::{debug, info};
use zeroize::Zeroizing;

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
/// Permissions of a written private key file.
const PRIVATE_KEY_MODE: u32 = 0o600;
/// Permissions of a written public key file.
const PUBLIC_KEY_MODE: u32 = 0o644;

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

/// Result of trying the credentials biwa normally uses for public-key authentication.
#[derive(Debug)]
enum AuthProbe {
	/// Key authentication already works.
	Works,
	/// No key file, agent identity, or configured credential could be used yet.
	NoCredentials(Report),
	/// Every available credential was rejected by the server.
	Rejected(Report),
}

impl SetupSsh {
	/// Runs the guided SSH key setup.
	pub(super) async fn run(self, quiet: bool) -> Result<()> {
		let config = Config::load()?;
		let interactive = stdin().is_terminal();
		let cli_key_path = self.key_path.as_deref().map(expand_key_path).transpose()?;
		let configured_key_path = config
			.ssh
			.key_path
			.as_deref()
			.map(expand_key_path)
			.transpose()?;
		let selected_key_path = cli_key_path.clone().or_else(|| configured_key_path.clone());

		// The probe reuses the ordinary public-key resolution, so an agent identity or an
		// `IdentityFile` entry counts as working authentication even without a local key file.
		let probe_config = public_key_config(&config, selected_key_path.clone());
		let target = resolve_key_target(&probe_config)?;

		if self.check {
			return run_check(&probe_config, &target, selected_key_path.as_deref(), quiet).await;
		}

		match probe_public_key_authentication(&probe_config, &target, quiet).await? {
			AuthProbe::Works => {
				report_ready(selected_key_path.as_deref(), quiet);
				self.finish_configuration(&config, selected_key_path.as_deref(), quiet);
				return Ok(());
			}
			AuthProbe::NoCredentials(error) => {
				debug!(error = %error, "No public-key credential is available yet");
			}
			AuthProbe::Rejected(error) => {
				debug!(error = %error, "Key authentication is not authorized yet");
			}
		}

		let (key_path, key_source) = select_key_path(
			cli_key_path,
			configured_key_path,
			&existing_default_key_paths(),
			generated_key_path(),
		)?;
		debug!(path = %key_path.display(), source = key_source.describe(), "Selected the SSH key to install");

		let generated = !key_path.is_file();
		let public_key = if generated {
			self.create_key_pair(&key_path, key_source, &target, interactive, quiet)?
		} else {
			load_key_pair(&key_path)?
		};

		// The key selected for installation may conflict with the OpenSSH `IdentityFile`
		// entry, which the ambient probe could not see. Checking here keeps that local
		// conflict from surfacing only after authorized_keys was already changed.
		let key_config = public_key_config(&config, Some(key_path.clone()));
		let install_target = resolve_key_target(&key_config).inspect_err(|_error| {
			if generated {
				report_unused_key(&key_path, quiet);
			}
		})?;

		let authorized_key = authorized_key_line(&public_key)?;
		if !quiet {
			eprintln!(
				"Installing {} on {}@{}",
				style(key_fingerprint(&public_key)).bold(),
				install_target.user,
				install_target.hostname
			);
		}

		let client = connect_with_password(&config, quiet).await?;
		let outcome = install_authorized_key(&client, &authorized_key).await?;
		drop(client);
		report_install_outcome(outcome, quiet);

		match probe_public_key_authentication(&key_config, &install_target, quiet).await? {
			AuthProbe::Works => {}
			AuthProbe::NoCredentials(error) => {
				return Err(error).wrap_err(format!(
					"The public key was installed, but {} could not be used as a credential",
					key_path.display()
				));
			}
			AuthProbe::Rejected(error) => {
				return Err(error).wrap_err(format!(
					"The public key was installed, but key authentication was still rejected. Check the server's `AuthorizedKeysFile` setting and the permissions of {REMOTE_SSH_DIR}"
				));
			}
		}
		report_ready(Some(&key_path), quiet);

		self.finish_configuration(&config, Some(&key_path), quiet);
		Ok(())
	}

	/// Records the selected key locally, or explains what to change by hand.
	///
	/// Configuration bookkeeping never fails the command: the remote side is already set
	/// up at this point, so a configuration file biwa cannot rewrite only downgrades to a
	/// printed snippet.
	fn finish_configuration(&self, config: &Config, key_path: Option<&Path>, quiet: bool) {
		if !self.write_config {
			warn_about_password_mode(config, key_path, quiet);
			return;
		}

		let Some(key_path) = key_path else {
			if !quiet {
				eprintln!(
					"{} --write-config needs a specific key, but authentication succeeded through your existing SSH configuration. Pass --key-path to record one",
					style("!").yellow().bold()
				);
			}
			warn_about_password_mode(config, key_path, quiet);
			return;
		};

		if let Err(error) = write_config_key_path(config, key_path, quiet) {
			debug!(error = ?error, "Failed to update the biwa configuration file");
			// Only the top-level context is printed: the chain below it is a parser dump.
			// The snippet already carries the `ssh.auth` change, so no extra warning follows.
			report_manual_config(config, key_path, &error.to_string(), quiet);
		}
	}

	/// Creates a key pair after confirming the choice when possible.
	fn create_key_pair(
		&self,
		key_path: &Path,
		key_source: KeySource,
		target: &ResolvedSshTarget,
		interactive: bool,
		quiet: bool,
	) -> Result<PublicKey> {
		if !self.generate {
			if !interactive {
				bail!(
					"No SSH private key was found at {} (selected by {}). Pass --generate to create one, or select an existing key with --key-path",
					key_path.display(),
					key_source.describe()
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
	target: &ResolvedSshTarget,
	key_path: Option<&Path>,
	quiet: bool,
) -> Result<()> {
	match probe_public_key_authentication(config, target, quiet).await? {
		AuthProbe::Works => {
			report_ready(key_path, quiet);
			Ok(())
		}
		AuthProbe::NoCredentials(error) => Err(error).wrap_err(format!(
			"No SSH key or agent identity could be used for {}@{}. Run `biwa setup-ssh --generate` to create one",
			target.user, target.hostname
		)),
		AuthProbe::Rejected(error) => Err(error).wrap_err(format!(
			"Key authentication for {}@{} is not working yet. Run `biwa setup-ssh` to install the matching public key",
			target.user, target.hostname
		)),
	}
}

/// Resolves the SSH target for one public-key configuration.
///
/// Every remote step runs after this, so a local conflict such as `ssh.key_path`
/// disagreeing with the OpenSSH `IdentityFile` entry stops the command before the remote
/// `authorized_keys` file is changed.
fn resolve_key_target(key_config: &Config) -> Result<ResolvedSshTarget> {
	ResolvedSshTarget::resolve(&key_config.ssh)
		.wrap_err("Failed to resolve the SSH target for key authentication")
}

/// Returns a configuration that authenticates with public keys only.
fn public_key_config(config: &Config, key_path: Option<PathBuf>) -> Config {
	let mut key_config = config.clone();
	key_config.ssh.auth = AuthMode::PublicKey;
	key_config.ssh.key_path = key_path;
	key_config
}

/// Reports that a key pair was created but not used.
fn report_unused_key(key_path: &Path, quiet: bool) {
	if quiet {
		return;
	}
	eprintln!(
		"{} The key pair generated at {} was not installed and can be removed",
		style("!").yellow().bold(),
		key_path.display()
	);
}

/// Reports that key authentication is ready.
fn report_ready(key_path: Option<&Path>, quiet: bool) {
	if quiet {
		return;
	}
	match key_path {
		Some(path) => eprintln!(
			"{} Key authentication works with {}",
			style("✓").green().bold(),
			path.display()
		),
		None => eprintln!(
			"{} Key authentication already works with your existing SSH credentials",
			style("✓").green().bold()
		),
	}
}

/// Reports whether the public key had to be added remotely.
fn report_install_outcome(outcome: InstallOutcome, quiet: bool) {
	if quiet {
		return;
	}
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

/// Warns when configuration still forces password authentication.
fn warn_about_password_mode(config: &Config, key_path: Option<&Path>, quiet: bool) {
	if config.ssh.auth != AuthMode::Password || quiet {
		return;
	}
	let target = key_path.map_or_else(|| "your key".to_owned(), |path| path.display().to_string());
	eprintln!(
		"{} `ssh.auth` is still set to \"password\". Remove it, or run `biwa setup-ssh --write-config`, to use {target}",
		style("!").yellow().bold()
	);
}

/// Tries the public-key credentials the configuration selects.
///
/// Only a rejected credential means "not authorized yet". Host-key failures, unusable
/// credentials, and local configuration errors abort so nothing remote is changed while a
/// local problem is misread as a missing authorization.
async fn probe_public_key_authentication(
	config: &Config,
	target: &ResolvedSshTarget,
	quiet: bool,
) -> Result<AuthProbe> {
	if let Err(error) = resolve_auth(config, target).await {
		debug!(error = %error, "No public-key credential could be resolved");
		return Ok(AuthProbe::NoCredentials(error));
	}

	match connect(config, quiet).await {
		Ok(client) => {
			drop(client);
			Ok(AuthProbe::Works)
		}
		Err(error) if error.downcast_ref::<HostKeyVerificationFailed>().is_some() => Err(error),
		Err(error) => match authentication_failure_kind(&error) {
			Some(AuthenticationFailureKind::Retryable) => Ok(AuthProbe::Rejected(error)),
			Some(AuthenticationFailureKind::Terminal) | None => Err(error),
		},
	}
}

/// Returns the fallback policy attached to an authentication failure marker.
fn authentication_failure_kind(report: &Report) -> Option<AuthenticationFailureKind> {
	report
		.downcast_ref::<AuthenticationFailed>()
		.map(|failure| failure.kind())
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
///
/// The script is POSIX shell, so a login shell such as `csh` or `fish` cannot run it.
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
		format!("if grep -q -E -e {quoted_pattern} {REMOTE_AUTHORIZED_KEYS}; then"),
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
	let line = line.trim().to_owned();
	// One key must authorize exactly one entry, whatever the comment happens to hold.
	if line.contains(['\n', '\r']) {
		bail!(
			"The public key comment spans several lines; refusing to write an ambiguous authorized_keys entry"
		)
	}
	Ok(line)
}

/// Builds the pattern matching an `authorized_keys` entry that authorizes this key.
///
/// The entry must begin with the algorithm, optionally indented, so a commented-out line
/// or an entry carrying options such as `from="..."` is not mistaken for an authorization
/// this command can rely on. The comment at the end of the entry is ignored, so a key
/// installed under a different comment is still recognized.
fn authorized_key_pattern(authorized_key: &str) -> Result<String> {
	let mut fields = authorized_key.split_whitespace();
	let (Some(algorithm), Some(material)) = (fields.next(), fields.next()) else {
		bail!("Malformed public key line: expected an algorithm and key material")
	};
	Ok(format!(
		"^[[:space:]]*{} {}([[:space:]]|$)",
		escape_pattern(algorithm),
		escape_pattern(material)
	))
}

/// Escapes the characters that are special inside a POSIX extended regular expression.
fn escape_pattern(value: &str) -> String {
	value
		.chars()
		.flat_map(|character| {
			let escape = matches!(
				character,
				'\\' | '.' | '[' | ']' | '(' | ')' | '{' | '}' | '*' | '+' | '?' | '|' | '^' | '$'
			);
			escape.then_some('\\').into_iter().chain([character])
		})
		.collect()
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
	new_key_path
		.map(|path| (path, KeySource::NewKey))
		.ok_or_else(|| {
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
	// Parsing the file directly keeps the comment, which `load_public_key` discards. Only
	// the first line is read: a comment may contain spaces, so anything after it would be
	// folded into the comment and turn one selected key into several authorized entries.
	if let Some(public_key) = fs::read_to_string(&companion)
		.ok()
		.and_then(|contents| PublicKey::from_openssh(contents.lines().next()?.trim()).ok())
	{
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
	let public_key_path = public_key_path(private_key_path);
	// `symlink_metadata` does not follow links, so a dangling symlink is refused instead of
	// being followed. The files are created exclusively below, which closes the remaining
	// window between this check and the write.
	for path in [private_key_path, public_key_path.as_path()] {
		if path.symlink_metadata().is_ok() {
			bail!("Refusing to overwrite the existing file {}", path.display());
		}
	}
	create_key_directory(private_key_path)?;

	let mut private_key = match key_type {
		KeyType::Ed25519 => {
			let seed = random_bytes::<32>()?;
			PrivateKey::from(Ed25519Keypair::from_seed(&seed))
		}
	};
	private_key.set_comment(comment);
	let public_key = private_key.public_key().clone();

	let passphrase = Zeroizing::new(if interactive {
		read_new_passphrase()?
	} else {
		String::new()
	});
	let stored_key = if passphrase.is_empty() {
		private_key
	} else {
		encrypt_private_key(&private_key, &passphrase)?
	};

	let encoded_private_key = stored_key
		.to_openssh(LineEnding::LF)
		.wrap_err("Failed to encode the generated private key")?;
	write_new_file(private_key_path, &encoded_private_key, PRIVATE_KEY_MODE)?;

	let encoded_public_key = public_key
		.to_openssh()
		.wrap_err("Failed to encode the generated public key")?;
	write_new_file(
		&public_key_path,
		&format!("{encoded_public_key}\n"),
		PUBLIC_KEY_MODE,
	)?;

	Ok(public_key)
}

/// Creates a file that must not exist yet, with the given Unix permissions.
#[cfg_attr(
	not(unix),
	expect(unused_variables, reason = "file modes only exist on Unix")
)]
fn write_new_file(path: &Path, contents: &str, mode: u32) -> Result<()> {
	let mut options = OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt as _;

		options.mode(mode);
	}

	let mut file = options
		.open(path)
		.wrap_err_with(|| format!("Failed to create {}", path.display()))?;
	file.write_all(contents.as_bytes())
		.wrap_err_with(|| format!("Failed to write {}", path.display()))
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

/// Returns cryptographically secure random bytes, cleared when they go out of scope.
fn random_bytes<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
	let mut bytes = Zeroizing::new([0_u8; N]);
	getrandom::fill(bytes.as_mut()).wrap_err("Failed to read random bytes for key generation")?;
	Ok(bytes)
}

/// Writes the selected key into the nearest local configuration file.
fn write_config_key_path(config: &Config, key_path: &Path, quiet: bool) -> Result<()> {
	let display_path = home_relative_path(key_path);
	let Some((config_path, format)) = Config::find_nearest_config_file()? else {
		report_manual_config(
			config,
			key_path,
			"no biwa configuration file was found",
			quiet,
		);
		return Ok(());
	};
	if format != ConfigFormat::Toml {
		report_manual_config(
			config,
			key_path,
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
fn report_manual_config(config: &Config, key_path: &Path, reason: &str, quiet: bool) {
	if quiet {
		return;
	}
	eprintln!(
		"{} Could not update the biwa configuration automatically: {reason}",
		style("!").yellow().bold()
	);
	eprintln!(
		"  Add this to your configuration manually:\n\n{}\n",
		manual_config_snippet(config.ssh.auth, key_path)
	);
}

/// Renders the configuration snippet that selects the installed key.
fn manual_config_snippet(auth: AuthMode, key_path: &Path) -> String {
	let encoded_path = toml::Value::String(home_relative_path(key_path)).to_string();
	let auth_line = if auth == AuthMode::Password {
		let encoded_auth = toml::Value::String(AuthMode::PublicKey.as_str().to_owned()).to_string();
		format!("\nauth = {encoded_auth}")
	} else {
		String::new()
	};
	format!("[ssh]\nkey_path = {encoded_path}{auth_line}")
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
/// Only the affected line is rewritten so comments, ordering, formatting, and the file's
/// line endings survive.
fn set_toml_ssh_value(contents: &str, key: &str, value: &str) -> Result<String> {
	let encoded = toml::Value::String(value.to_owned()).to_string();
	let dotted_key = format!("ssh.{key}");
	// `str::lines` already drops a carriage return, so only the joining style has to match.
	let line_ending = if contents.contains("\r\n") {
		"\r\n"
	} else {
		"\n"
	};
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
		let Some((name, assigned)) = assignment(trimmed) else {
			continue;
		};
		let matches = match current_table.as_deref() {
			Some("[ssh]") => name == key,
			None => name == dotted_key,
			Some(_) => false,
		};
		if matches {
			let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
			let comment = trailing_comment(assigned)
				.map(|comment| format!(" {comment}"))
				.unwrap_or_default();
			*line = format!("{indent}{name} = {encoded}{comment}");
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

	let mut updated = lines.join(line_ending);
	if contents.is_empty() || contents.ends_with('\n') {
		updated.push_str(line_ending);
	}
	verify_toml_ssh_value(&updated, key, value)?;
	Ok(updated)
}

/// Splits a `key = value` assignment line into its key and everything after the `=`.
fn assignment(line: &str) -> Option<(&str, &str)> {
	let (key, assigned) = line.split_once('=')?;
	let key = key.trim();
	if key.is_empty() || key.contains(char::is_whitespace) {
		return None;
	}
	Some((key, assigned))
}

/// Returns the comment trailing an assigned TOML value, if any.
///
/// A `#` inside a quoted string starts no comment, so quoting is tracked while scanning.
fn trailing_comment(assigned: &str) -> Option<&str> {
	let mut quote: Option<char> = None;
	let mut escaped = false;

	for (index, character) in assigned.char_indices() {
		if escaped {
			escaped = false;
			continue;
		}
		match (quote, character) {
			// Only a basic string uses backslash escapes.
			(Some('"'), '\\') => escaped = true,
			(Some(open), _) if open == character => quote = None,
			(None, '"' | '\'') => quote = Some(character),
			(None, '#') => return assigned.get(index..).map(str::trim_end),
			(Some(_) | None, _) => {}
		}
	}
	None
}

/// Ensures the rewritten configuration parses and selects the intended value.
fn verify_toml_ssh_value(contents: &str, key: &str, value: &str) -> Result<()> {
	let parsed: toml::Value = toml::from_str(contents).wrap_err_with(|| {
		format!("biwa cannot rewrite ssh.{key} in this file layout, so it was left unchanged")
	})?;
	let updated = parsed
		.get("ssh")
		.and_then(|ssh| ssh.get(key))
		.and_then(toml::Value::as_str);
	if updated == Some(value) {
		return Ok(());
	}
	bail!("biwa cannot set ssh.{key} without rewriting unrelated configuration")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::{Cli, Commands};
	use crate::testing::write_test_ssh_private_key;
	use pretty_assertions::assert_eq;

	const PUBLIC_KEY: &str =
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T biwa";
	const KEY_MATERIAL: &str =
		"AAAAC3NzaC1lZDI1NTE5AAAAIGYh1Ntz2neFcfgNyBAx3kFJwSURKqRrnAuLiQ5M296T";

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
	fn public_key_config_forces_public_key_authentication() {
		let mut config = Config::default();
		config.ssh.auth = AuthMode::Password;
		config.ssh.key_path = Some(PathBuf::from("/configured"));

		let selected = public_key_config(&config, Some(PathBuf::from("/selected")));
		assert_eq!(selected.ssh.auth, AuthMode::PublicKey);
		assert_eq!(selected.ssh.key_path, Some(PathBuf::from("/selected")));

		let ambient = public_key_config(&config, None);
		assert_eq!(ambient.ssh.auth, AuthMode::PublicKey);
		assert_eq!(ambient.ssh.key_path, None);
	}

	#[test]
	fn authentication_failure_kind_reads_the_marker() {
		let rejected =
			Report::from(AuthenticationFailed::retryable()).wrap_err("credential rejected");
		let terminal = Report::from(AuthenticationFailed::terminal()).wrap_err("unusable key");

		assert_eq!(
			authentication_failure_kind(&rejected),
			Some(AuthenticationFailureKind::Retryable)
		);
		assert_eq!(
			authentication_failure_kind(&terminal),
			Some(AuthenticationFailureKind::Terminal)
		);
		assert_eq!(
			authentication_failure_kind(&Report::msg("local error")),
			None
		);
	}

	#[test]
	fn authorized_key_line_drops_trailing_whitespace() -> Result<()> {
		assert_eq!(authorized_key_line(&public_key())?, PUBLIC_KEY);
		Ok(())
	}

	#[test]
	fn authorized_key_pattern_anchors_the_entry_and_ignores_the_comment() -> Result<()> {
		assert_eq!(
			authorized_key_pattern(PUBLIC_KEY)?,
			format!("^[[:space:]]*ssh-ed25519 {KEY_MATERIAL}([[:space:]]|$)")
		);
		let _error =
			authorized_key_pattern("ssh-ed25519").expect_err("a key line needs key material");
		Ok(())
	}

	#[test]
	fn escape_pattern_protects_regex_metacharacters() {
		assert_eq!(escape_pattern("ssh-ed25519"), "ssh-ed25519");
		assert_eq!(
			escape_pattern("sk-ssh-ed25519@openssh.com"),
			"sk-ssh-ed25519@openssh\\.com"
		);
		assert_eq!(escape_pattern("a+b/c=d"), "a\\+b/c=d");
		assert_eq!(escape_pattern("^$.[]()"), "\\^\\$\\.\\[\\]\\(\\)");
	}

	#[test]
	fn install_script_quotes_the_key_and_is_idempotent() -> Result<()> {
		let script =
			install_authorized_key_script(PUBLIC_KEY, &authorized_key_pattern(PUBLIC_KEY)?);

		assert!(script.contains("mkdir -p ~/.ssh"));
		assert!(script.contains("chmod 700 ~/.ssh"));
		assert!(script.contains("chmod 600 ~/.ssh/authorized_keys"));
		assert!(script.contains("if grep -q -E -e '^[[:space:]]*ssh-ed25519 "));
		assert!(script.contains(&format!("printf '%s\\n' '{PUBLIC_KEY}'")));
		assert!(script.contains(ALREADY_PRESENT_MARKER));
		assert!(script.contains(INSTALLED_MARKER));
		Ok(())
	}

	#[test]
	fn install_script_quotes_hostile_comments() -> Result<()> {
		let hostile = format!("{PUBLIC_KEY}; rm -rf ~");
		let script = install_authorized_key_script(&hostile, &authorized_key_pattern(&hostile)?);

		assert!(script.contains(&format!("printf '%s\\n' '{hostile}'")));
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
	fn expand_key_path_resolves_the_home_marker() -> Result<()> {
		if let Some(home) = homedir::my_home().ok().flatten() {
			assert_eq!(
				expand_key_path(Path::new("~/.ssh/id_ed25519"))?,
				home.join(".ssh/id_ed25519")
			);
		}
		assert_eq!(
			expand_key_path(Path::new("/opt/keys/id_ed25519"))?,
			PathBuf::from("/opt/keys/id_ed25519")
		);
		Ok(())
	}

	#[test]
	fn existing_key_pair_keeps_the_public_key_comment() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let key_path = dir.path().join("id_ed25519");
		write_test_ssh_private_key(&key_path)?;
		fs::write(public_key_path(&key_path), format!("{PUBLIC_KEY}\n"))?;

		assert_eq!(
			authorized_key_line(&load_key_pair(&key_path)?)?,
			PUBLIC_KEY,
			"the comment stored beside an existing key must reach authorized_keys"
		);
		Ok(())
	}

	#[test]
	fn only_the_first_entry_of_a_public_key_file_is_installed() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let key_path = dir.path().join("id_ed25519");
		let expected = write_test_ssh_private_key(&key_path)?;
		// A comment may contain spaces, so a second line would otherwise be folded into the
		// comment and authorize an unrelated key.
		fs::write(
			public_key_path(&key_path),
			format!("{} biwa-e2e\n{PUBLIC_KEY}\n", expected.to_openssh()?),
		)?;

		let loaded = load_key_pair(&key_path)?;
		assert_eq!(loaded.key_data(), expected.key_data());
		let line = authorized_key_line(&loaded)?;
		assert!(!line.contains('\n'), "line was: {line}");
		assert!(
			!line.contains("AAAAC3NzaC1lZDI1NTE5AAAAIGYh"),
			"the unrelated key must not be authorized: {line}"
		);
		Ok(())
	}

	#[test]
	fn authorized_key_line_rejects_a_multiline_comment() {
		let mut public_key = public_key();
		public_key.set_comment("first\nssh-ed25519 AAAAsecond");

		let error = authorized_key_line(&public_key)
			.expect_err("a comment spanning lines cannot become one entry");
		assert!(error.to_string().contains("spans several lines"));
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
				PRIVATE_KEY_MODE,
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

	#[cfg(unix)]
	#[test]
	fn generated_key_pair_refuses_to_follow_a_symlink() -> Result<()> {
		use std::os::unix::fs::symlink;

		let dir = tempfile::tempdir()?;
		let key_path = dir.path().join("id_ed25519");
		let target = dir.path().join("elsewhere");
		symlink(&target, &key_path)?;

		let error = generate_key_pair(&key_path, KeyType::Ed25519, "biwa@test", false)
			.expect_err("a dangling symlink must not be followed");
		assert!(error.to_string().contains("Refusing to overwrite"));
		assert!(!target.exists(), "the symlink target must not be created");
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
	fn toml_value_preserves_a_trailing_comment() -> Result<()> {
		let updated = set_toml_ssh_value(
			"[ssh]\nkey_path = \"~/.ssh/old\" # the key biwa uses\n",
			"key_path",
			"~/.ssh/new",
		)?;

		assert_eq!(
			updated,
			"[ssh]\nkey_path = \"~/.ssh/new\" # the key biwa uses\n"
		);
		Ok(())
	}

	#[test]
	fn toml_value_ignores_a_hash_inside_the_old_value() -> Result<()> {
		let updated = set_toml_ssh_value(
			"[ssh]\nkey_path = \"~/.ssh/id#1\"\n",
			"key_path",
			"~/.ssh/new",
		)?;

		assert_eq!(updated, "[ssh]\nkey_path = \"~/.ssh/new\"\n");
		Ok(())
	}

	#[test]
	fn toml_value_preserves_carriage_returns() -> Result<()> {
		let updated = set_toml_ssh_value(
			"[ssh]\r\nhost = \"cse\"\r\n",
			"key_path",
			"~/.ssh/id_ed25519",
		)?;

		assert_eq!(
			updated,
			"[ssh]\r\nkey_path = \"~/.ssh/id_ed25519\"\r\nhost = \"cse\"\r\n"
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
		assert!(error.to_string().contains("cannot rewrite ssh.key_path"));
	}

	#[test]
	fn assignment_reads_only_simple_assignments() {
		assert_eq!(
			assignment("key_path = \"value\""),
			Some(("key_path", " \"value\""))
		);
		assert_eq!(
			assignment("ssh.key_path=\"value\""),
			Some(("ssh.key_path", "\"value\""))
		);
		assert_eq!(assignment("no assignment here"), None);
		assert_eq!(assignment("two words = 1"), None);
		assert_eq!(assignment("= 1"), None);
	}

	#[test]
	fn trailing_comment_skips_quoted_hashes() {
		assert_eq!(trailing_comment(" \"value\" # comment "), Some("# comment"));
		assert_eq!(trailing_comment(" \"va#lue\""), None);
		assert_eq!(trailing_comment(" '#literal'"), None);
		assert_eq!(trailing_comment(" \"escaped \\\" # inside\""), None);
		assert_eq!(trailing_comment(" true"), None);
	}

	#[test]
	fn manual_snippet_encodes_the_path_and_the_auth_change() {
		// A path containing a quote becomes a TOML literal string rather than an invalid
		// basic string.
		assert_eq!(
			manual_config_snippet(AuthMode::PublicKey, Path::new("/opt/keys/id\"quoted")),
			"[ssh]\nkey_path = '/opt/keys/id\"quoted'"
		);
		assert_eq!(
			manual_config_snippet(AuthMode::Password, Path::new("/opt/keys/id_ed25519")),
			"[ssh]\nkey_path = \"/opt/keys/id_ed25519\"\nauth = \"public-key\""
		);
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
