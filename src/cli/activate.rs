use crate::Result;
use crate::cli::run::validate_direct_options;
use crate::config::types::Config;
use alloc::collections::BTreeSet;
use clap::{Args, Subcommand, ValueEnum};
use color_eyre::eyre::{WrapErr as _, bail};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

/// File recording the shim names created by biwa in a configured directory.
const MANAGED_SHIMS_FILE: &str = ".biwa-managed-shims";

/// Shell activation and direct command shim management.
#[derive(Args, Debug)]
pub(super) struct Activate {
	/// Print activation code for this shell.
	#[arg(long, value_enum)]
	shell: Option<ActivationShell>,

	/// Activation command to run.
	#[command(subcommand)]
	command: Option<ActivateCommand>,
}

/// Supported activation commands.
#[derive(Subcommand, Debug)]
enum ActivateCommand {
	/// Reconcile configured direct command shims.
	Install(Install),
	/// Print diagnostic information for direct command activation.
	Doctor,
}

/// Direct command shim installation options.
#[derive(Args, Debug)]
struct Install {
	/// Replace existing entries not already managed by biwa.
	#[arg(long, short)]
	force: bool,
}

/// Shells that can receive activation code.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationShell {
	/// Bash shell.
	Bash,
	/// Zsh shell.
	Zsh,
	/// Fish shell.
	Fish,
}

/// Result from a shim installation run.
#[derive(Debug, Default, PartialEq, Eq)]
struct InstallReport {
	/// Newly installed or updated shim paths.
	installed: Vec<PathBuf>,
	/// Existing shim paths that already pointed at the current executable.
	unchanged: Vec<PathBuf>,
	/// Stale managed shim paths that were removed.
	removed: Vec<PathBuf>,
}

/// Result from creating or updating one shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShimInstallStatus {
	/// The shim was created or updated.
	Installed,
	/// The shim already pointed at the current executable.
	Unchanged,
}

impl Activate {
	/// Runs the activation command.
	pub(super) fn run(self) -> Result<()> {
		let config = Config::load_global_optional_ssh()?;
		let bin_dir = config.direct.resolved_bin_dir();
		let mut did_work = false;

		match self.command {
			Some(ActivateCommand::Install(install)) => {
				let report = install_shims(&config, &env::current_exe()?, install.force)?;
				print_install_report(&report);
				did_work = true;
			}
			Some(ActivateCommand::Doctor) => {
				print_doctor(&config)?;
				did_work = true;
			}
			None => {}
		}

		if let Some(shell) = self.shell {
			ensure_secure_shim_dir(&bin_dir)?;
			println!("{}", activation_script(shell, &bin_dir)?);
			did_work = true;
		}

		if !did_work {
			bail!("Specify `--shell <bash|zsh|fish>`, `install`, or `doctor`.");
		}
		Ok(())
	}
}

/// Expands a symlink invocation into the equivalent normal `biwa run` argv.
pub(super) fn expand_direct_invocation(
	args: impl IntoIterator<Item = OsString>,
) -> Result<Vec<OsString>> {
	let args = args.into_iter().collect::<Vec<_>>();
	let Some(argv0) = args.first() else {
		return Ok(args);
	};
	let Some(command) = direct_command_name(argv0) else {
		return Ok(args);
	};

	let config = Config::load_global_optional_ssh()?;
	expand_direct_invocation_with_config(args, &command, &config)
}

/// Expands a known direct command using its global configuration.
fn expand_direct_invocation_with_config(
	args: Vec<OsString>,
	command: &str,
	config: &Config,
) -> Result<Vec<OsString>> {
	let options = config.direct.commands.get(command).ok_or_else(|| {
		color_eyre::eyre::eyre!(
			"Direct command `{command}` is not configured in global `direct.commands`."
		)
	})?;
	validate_direct_command(command, options)?;

	let mut expanded = Vec::new();
	expanded.push(OsString::from("biwa"));
	expanded.push(OsString::from("run"));
	expanded.extend(options.iter().map(OsString::from));
	expanded.push(OsString::from(format!("'{command}'")));
	expanded.extend(args.into_iter().skip(1));
	Ok(expanded)
}

/// Returns the shim command name from `argv[0]`, excluding normal biwa invocations.
fn direct_command_name(argv0: &OsStr) -> Option<String> {
	let name = Path::new(argv0).file_name()?.to_str()?;
	if is_biwa_binary_name(name) {
		return None;
	}
	Some(name.to_owned())
}

/// Returns whether an executable basename is a normal biwa binary name.
fn is_biwa_binary_name(name: &str) -> bool {
	matches!(name, "biwa" | "biwa.exe")
}

/// Returns shell code that appends the shim directory to PATH.
fn activation_script(shell: ActivationShell, bin_dir: &Path) -> Result<String> {
	let bin_dir = bin_dir.to_str().ok_or_else(|| {
		color_eyre::eyre::eyre!(
			"Direct shim directory `{}` is not valid UTF-8 and cannot be added to PATH",
			bin_dir.display()
		)
	})?;
	let quoted_bin_dir = shell_words::quote(bin_dir);

	Ok(match shell {
		ActivationShell::Bash | ActivationShell::Zsh => format!(
			r#"__biwa_direct_bin={quoted_bin_dir}
__biwa_direct_path=$PATH
while :; do
  case "$__biwa_direct_path" in
    "$__biwa_direct_bin") __biwa_direct_path= ;;
    "$__biwa_direct_bin":*) __biwa_direct_path=${{__biwa_direct_path#*:}} ;;
    *:"$__biwa_direct_bin":*) __biwa_direct_path=${{__biwa_direct_path/:"$__biwa_direct_bin":/:}} ;;
    *:"$__biwa_direct_bin") __biwa_direct_path=${{__biwa_direct_path%:*}} ;;
    *) break ;;
  esac
done
if [ -n "$__biwa_direct_path" ]; then
  export PATH="$__biwa_direct_path:$__biwa_direct_bin"
else
  export PATH="$__biwa_direct_bin"
fi
unset __biwa_direct_path
unset __biwa_direct_bin"#
		),
		ActivationShell::Fish => format!(
			"set -l __biwa_direct_bin {quoted_bin_dir}
set -l __biwa_direct_path
for __biwa_direct_entry in $PATH
  if test \"$__biwa_direct_entry\" != \"$__biwa_direct_bin\"
    set -a __biwa_direct_path \"$__biwa_direct_entry\"
  end
end
set -gx PATH $__biwa_direct_path $__biwa_direct_bin
set -e __biwa_direct_entry
set -e __biwa_direct_path
set -e __biwa_direct_bin"
		),
	})
}

/// Reconciles the configured direct command shims.
fn install_shims(config: &Config, biwa_path: &Path, force: bool) -> Result<InstallReport> {
	let mut report = InstallReport::default();
	let bin_dir = config.direct.resolved_bin_dir();
	let shim_names = configured_shim_names(config)?;
	let desired_shims = shim_names.iter().cloned().collect::<BTreeSet<_>>();

	ensure_secure_shim_dir(&bin_dir)?;
	let managed_shims = read_managed_shims(&bin_dir)?;
	for name in managed_shims.difference(&desired_shims) {
		let path = bin_dir.join(name);
		match fs::symlink_metadata(&path) {
			Ok(_) if is_managed_symlink(&path)? => {
				fs::remove_file(&path).wrap_err_with(|| {
					format!(
						"Failed to remove stale direct command shim `{}`",
						path.display()
					)
				})?;
				report.removed.push(path);
			}
			Ok(_) => {}
			Err(error) if error.kind() == ErrorKind::NotFound => {}
			Err(error) => {
				return Err(error).wrap_err_with(|| {
					format!(
						"Failed to inspect stale direct command shim `{}`",
						path.display()
					)
				});
			}
		}
	}

	for command in shim_names {
		let was_managed = managed_shims.contains(&command);
		let shim_path = bin_dir.join(command);
		match create_or_update_symlink(&shim_path, biwa_path, force, was_managed)? {
			ShimInstallStatus::Installed => report.installed.push(shim_path),
			ShimInstallStatus::Unchanged => report.unchanged.push(shim_path),
		}
	}
	write_managed_shims(&bin_dir, &desired_shims)?;
	Ok(report)
}

/// Prints a concise shim installation report to stderr.
fn print_install_report(report: &InstallReport) {
	for path in &report.installed {
		eprintln!("Installed direct command shim: {}", path.display());
	}
	for path in &report.unchanged {
		eprintln!("Direct command shim already current: {}", path.display());
	}
	for path in &report.removed {
		eprintln!("Removed stale direct command shim: {}", path.display());
	}
	if report.installed.is_empty() && report.unchanged.is_empty() && report.removed.is_empty() {
		eprintln!("No direct command shims configured in global `direct.commands`.");
	}
}

/// Prints direct command diagnostics.
fn print_doctor(config: &Config) -> Result<()> {
	configured_shim_names(config)?;
	println!(
		"direct.bin_dir = {}",
		config.direct.resolved_bin_dir().display()
	);
	println!("direct.commands = {}", config.direct.commands.len());
	for command in config.direct.commands.keys() {
		println!("- {command}");
	}
	Ok(())
}

/// Returns configured shim names after validating their names and options.
fn configured_shim_names(config: &Config) -> Result<Vec<String>> {
	for (name, options) in &config.direct.commands {
		validate_direct_command(name, options)?;
	}
	Ok(config.direct.commands.keys().cloned().collect())
}

/// Validates one exact direct command entry.
fn validate_direct_command(name: &str, options: &[String]) -> Result<()> {
	validate_shim_name(name)?;
	validate_direct_options(name, options)
}

/// Rejects shim names that could resolve outside the configured shim directory.
fn validate_shim_name(name: &str) -> Result<()> {
	if name.is_empty()
		|| name.starts_with('-')
		|| name.starts_with(".biwa-")
		|| is_biwa_binary_name(name)
		|| Path::new(name).file_name() != Some(OsStr::new(name))
		|| !name
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
	{
		bail!(
			"Invalid direct command shim name `{name}`: use only ASCII letters, digits, `-`, `_`, `.`, or `+`; `biwa`, `biwa.exe`, and `.biwa-*` are reserved"
		);
	}
	Ok(())
}

/// Reads the names installed by an earlier reconciliation.
fn read_managed_shims(bin_dir: &Path) -> Result<BTreeSet<String>> {
	let manifest_path = bin_dir.join(MANAGED_SHIMS_FILE);
	let metadata = match fs::symlink_metadata(&manifest_path) {
		Ok(metadata) => metadata,
		Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeSet::new()),
		Err(error) => {
			return Err(error).wrap_err_with(|| {
				format!(
					"Failed to inspect direct shim manifest `{}`",
					manifest_path.display()
				)
			});
		}
	};
	if !metadata.is_file() || metadata.file_type().is_symlink() {
		bail!(
			"Direct shim manifest `{}` is not a regular file",
			manifest_path.display()
		);
	}

	let contents = fs::read_to_string(&manifest_path).wrap_err_with(|| {
		format!(
			"Failed to read direct shim manifest `{}`",
			manifest_path.display()
		)
	})?;
	let mut names = BTreeSet::new();
	for name in contents.lines() {
		validate_shim_name(name).wrap_err("Invalid direct shim manifest")?;
		names.insert(name.to_owned());
	}
	Ok(names)
}

/// Atomically records the names managed by this biwa installation.
fn write_managed_shims(bin_dir: &Path, names: &BTreeSet<String>) -> Result<()> {
	let manifest_path = bin_dir.join(MANAGED_SHIMS_FILE);
	let temporary_path = random_internal_path(bin_dir, "manifest-tmp")?;
	let mut contents = names.iter().cloned().collect::<Vec<_>>().join("\n");
	if !contents.is_empty() {
		contents.push('\n');
	}
	let mut options = fs::OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt as _;
		options.mode(0o600);
	}
	let mut temporary_file = options.open(&temporary_path).wrap_err_with(|| {
		format!(
			"Failed to create temporary direct shim manifest `{}`",
			temporary_path.display()
		)
	})?;
	temporary_file
		.write_all(contents.as_bytes())
		.wrap_err_with(|| {
			format!(
				"Failed to write temporary direct shim manifest `{}`",
				temporary_path.display()
			)
		})?;
	temporary_file.sync_all().wrap_err_with(|| {
		format!(
			"Failed to sync temporary direct shim manifest `{}`",
			temporary_path.display()
		)
	})?;
	drop(temporary_file);
	if let Err(rename_error) = fs::rename(&temporary_path, &manifest_path) {
		if let Err(cleanup_error) = fs::remove_file(&temporary_path) {
			return Err(rename_error).wrap_err_with(|| {
				format!(
					"Failed to update direct shim manifest `{}` and remove temporary manifest `{}`: {cleanup_error}",
					manifest_path.display(),
					temporary_path.display(),
				)
			});
		}
		return Err(rename_error).wrap_err_with(|| {
			format!(
				"Failed to update direct shim manifest `{}`",
				manifest_path.display()
			)
		});
	}
	Ok(())
}

/// Returns an unpredictable path in the reserved internal filename namespace.
fn random_internal_path(bin_dir: &Path, kind: &str) -> Result<PathBuf> {
	let mut random = [0_u8; 8];
	getrandom::fill(&mut random).wrap_err("Failed to generate a direct shim temporary name")?;
	Ok(bin_dir.join(format!(".biwa-{kind}-{}", hex::encode(random))))
}

/// Returns whether a symlink points at a biwa executable.
fn is_managed_symlink(path: &Path) -> Result<bool> {
	let metadata = fs::symlink_metadata(path)
		.wrap_err_with(|| format!("Failed to inspect direct command shim `{}`", path.display()))?;
	if !metadata.file_type().is_symlink() {
		return Ok(false);
	}
	let target = fs::read_link(path)
		.wrap_err_with(|| format!("Failed to read direct command shim `{}`", path.display()))?;
	Ok(target
		.file_name()
		.and_then(OsStr::to_str)
		.is_some_and(is_biwa_binary_name))
}

/// Creates the shim directory privately and rejects unsafe existing permissions.
#[cfg(unix)]
fn ensure_secure_shim_dir(bin_dir: &Path) -> Result<()> {
	use nix::unistd::Uid;
	use std::os::unix::ffi::OsStrExt as _;
	use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

	if !bin_dir.is_absolute() {
		bail!("Refusing to use non-absolute direct shim directory");
	}
	if bin_dir.as_os_str().as_bytes().contains(&b':') {
		bail!("Refusing to use direct shim directory containing `:`");
	}
	if !bin_dir.try_exists().wrap_err_with(|| {
		format!(
			"Failed to inspect direct shim directory `{}`",
			bin_dir.display()
		)
	})? {
		let mut builder = fs::DirBuilder::new();
		builder.recursive(true).mode(0o700);
		builder.create(bin_dir).wrap_err_with(|| {
			format!(
				"Failed to create direct shim directory `{}`",
				bin_dir.display()
			)
		})?;
	}

	let metadata = fs::symlink_metadata(bin_dir).wrap_err_with(|| {
		format!(
			"Failed to inspect direct shim directory `{}`",
			bin_dir.display()
		)
	})?;
	if metadata.file_type().is_symlink() {
		bail!(
			"Refusing to use symlinked direct shim directory `{}`",
			bin_dir.display()
		);
	}
	if !metadata.is_dir() {
		bail!(
			"Direct shim path `{}` is not a directory",
			bin_dir.display()
		);
	}
	let effective_uid = Uid::effective().as_raw();
	if metadata.uid() != effective_uid {
		bail!(
			"Refusing to use direct shim directory `{}` because it is not owned by the current user",
			bin_dir.display()
		);
	}
	if metadata.permissions().mode() & 0o022 != 0 {
		bail!(
			"Refusing to use group- or world-writable direct shim directory `{}`",
			bin_dir.display()
		);
	}

	let canonical_bin_dir = bin_dir.canonicalize().wrap_err_with(|| {
		format!(
			"Failed to resolve direct shim directory `{}`",
			bin_dir.display()
		)
	})?;
	for parent in canonical_bin_dir.ancestors().skip(1) {
		let parent_metadata = parent.metadata().wrap_err_with(|| {
			format!(
				"Failed to inspect parent of direct shim directory `{}`",
				parent.display()
			)
		})?;
		let mode = parent_metadata.permissions().mode();
		if parent_metadata.uid() != effective_uid && parent_metadata.uid() != 0 {
			bail!(
				"Refusing to use direct shim directory `{}` under parent `{}` owned by another user",
				bin_dir.display(),
				parent.display(),
			);
		}
		if mode & 0o022 != 0 && mode & 0o1000 == 0 {
			bail!(
				"Refusing to use direct shim directory `{}` under replaceable parent `{}`",
				bin_dir.display(),
				parent.display(),
			);
		}
	}
	Ok(())
}

/// Creates the shim directory on platforms without Unix permission bits.
#[cfg(not(unix))]
fn ensure_secure_shim_dir(bin_dir: &Path) -> Result<()> {
	fs::create_dir_all(bin_dir).wrap_err_with(|| {
		format!(
			"Failed to create direct shim directory `{}`",
			bin_dir.display()
		)
	})
}

/// Creates or updates one symlink shim.
#[cfg(unix)]
fn create_or_update_symlink(
	shim_path: &Path,
	biwa_path: &Path,
	force: bool,
	was_managed: bool,
) -> Result<ShimInstallStatus> {
	use std::os::unix::fs::symlink;

	let replace = match fs::symlink_metadata(shim_path) {
		Ok(metadata) if metadata.file_type().is_symlink() => {
			if fs::read_link(shim_path).is_ok_and(|target| target == biwa_path) {
				return Ok(ShimInstallStatus::Unchanged);
			}
			if !was_managed && !force {
				bail!(
					"Refusing to replace existing untracked symlink `{}`. Use `--force` to replace it.",
					shim_path.display()
				);
			}
			true
		}
		Ok(_) => {
			if !force {
				bail!(
					"Refusing to replace existing non-symlink `{}`. Use `--force` to replace it.",
					shim_path.display()
				);
			}
			if shim_path.is_dir() {
				bail!(
					"Refusing to replace existing directory `{}`",
					shim_path.display()
				);
			}
			true
		}
		Err(error) if error.kind() == ErrorKind::NotFound => false,
		Err(error) => {
			return Err(error).wrap_err_with(|| {
				format!(
					"Failed to inspect direct command shim `{}`",
					shim_path.display()
				)
			});
		}
	};

	if replace {
		let parent = shim_path
			.parent()
			.ok_or_else(|| color_eyre::eyre::eyre!("Shim path has no parent directory"))?;
		let temporary_path = random_internal_path(parent, "shim-tmp")?;
		symlink(biwa_path, &temporary_path).wrap_err_with(|| {
			format!(
				"Failed to create temporary direct command shim `{}`",
				temporary_path.display()
			)
		})?;
		if let Err(rename_error) = fs::rename(&temporary_path, shim_path) {
			if let Err(cleanup_error) = fs::remove_file(&temporary_path) {
				return Err(rename_error).wrap_err_with(|| {
					format!(
						"Failed to replace direct command shim `{}` and remove temporary shim `{}`: {cleanup_error}",
						shim_path.display(),
						temporary_path.display(),
					)
				});
			}
			return Err(rename_error).wrap_err_with(|| {
				format!(
					"Failed to replace direct command shim `{}`",
					shim_path.display()
				)
			});
		}
	} else {
		symlink(biwa_path, shim_path).wrap_err_with(|| {
			format!(
				"Failed to create direct command shim `{}` -> `{}`",
				shim_path.display(),
				biwa_path.display()
			)
		})?;
	}
	Ok(ShimInstallStatus::Installed)
}

/// Creates or updates one symlink shim.
#[cfg(not(unix))]
fn create_or_update_symlink(
	_shim_path: &Path,
	_biwa_path: &Path,
	_force: bool,
	_was_managed: bool,
) -> Result<ShimInstallStatus> {
	bail!("Direct command shim installation is only supported on Unix-like systems");
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::collections::BTreeMap;
	use pretty_assertions::assert_eq;
	use tempfile::tempdir;

	fn direct_config(commands: BTreeMap<String, Vec<String>>, bin_dir: PathBuf) -> Config {
		let mut config = Config::default();
		config.direct.commands = commands;
		config.direct.bin_dir = Some(bin_dir);
		config
	}

	#[test]
	fn normal_biwa_invocation_is_unchanged() {
		let args = [
			OsString::from("/usr/bin/biwa"),
			OsString::from("run"),
			OsString::from("dcc"),
		];
		assert_eq!(
			direct_command_name(args.first().expect("argv contains executable")),
			None
		);
	}

	#[test]
	fn direct_invocation_expands_to_normal_run_arguments() -> Result<()> {
		let dir = tempdir()?;
		let commands = BTreeMap::from([(
			"dcc".to_owned(),
			vec!["--skip-sync".to_owned(), "--remote-dir=~/dcc".to_owned()],
		)]);
		let config = direct_config(commands, dir.path().join("bin"));
		let expanded = expand_direct_invocation_with_config(
			vec![
				OsString::from("/tmp/dcc"),
				OsString::from("-Wall"),
				OsString::from("main.c"),
			],
			"dcc",
			&config,
		)?;

		assert_eq!(
			expanded,
			[
				"biwa",
				"run",
				"--skip-sync",
				"--remote-dir=~/dcc",
				"'dcc'",
				"-Wall",
				"main.c",
			]
			.map(OsString::from)
		);
		Ok(())
	}

	#[test]
	fn unknown_direct_invocation_is_rejected() {
		let error = expand_direct_invocation_with_config(
			vec![OsString::from("/tmp/sh")],
			"sh",
			&Config::default(),
		)
		.expect_err("unknown direct command should fail");
		assert!(error.to_string().contains("global `direct.commands`"));
	}

	#[test]
	fn configured_commands_are_exact_and_sorted() -> Result<()> {
		let dir = tempdir()?;
		let commands = BTreeMap::from([
			("dcc".to_owned(), Vec::new()),
			("1511".to_owned(), vec!["--skip-sync".to_owned()]),
		]);
		let config = direct_config(commands, dir.path().join("bin"));
		assert_eq!(configured_shim_names(&config)?, vec!["1511", "dcc"]);
		Ok(())
	}

	#[test]
	fn invalid_names_and_non_option_arguments_are_rejected() {
		assert!(validate_shim_name("../dcc").is_err());
		assert!(validate_shim_name(".").is_err());
		assert!(validate_shim_name("..").is_err());
		assert!(validate_shim_name("safe;id").is_err());
		assert!(validate_shim_name("-dcc").is_err());
		assert!(validate_shim_name("biwa").is_err());
		assert!(validate_shim_name("biwa.exe").is_err());
		assert!(validate_shim_name(MANAGED_SHIMS_FILE).is_err());
		let error = validate_direct_options("dcc", &["other-command".to_owned()])
			.expect_err("positional options should be rejected");
		assert!(error.to_string().contains("must not include"));
	}

	#[test]
	fn activation_output_moves_shim_dir_to_path_end() -> Result<()> {
		let bash = activation_script(ActivationShell::Bash, Path::new("/tmp/biwa/bin"))?;
		assert!(bash.contains(r#"PATH="$__biwa_direct_path:$__biwa_direct_bin""#));
		assert!(!bash.contains(r#"PATH="$__biwa_direct_bin:$PATH""#));
		assert!(bash.contains(r#"*:"$__biwa_direct_bin":*)"#));

		let fish = activation_script(ActivationShell::Fish, Path::new("/tmp/biwa/bin"))?;
		assert!(fish.contains("set -gx PATH $__biwa_direct_path $__biwa_direct_bin"));
		Ok(())
	}

	#[test]
	fn reserved_shell_word_is_quoted_for_remote_execution() -> Result<()> {
		let dir = tempdir()?;
		let config = direct_config(
			BTreeMap::from([("if".to_owned(), Vec::new())]),
			dir.path().join("bin"),
		);
		let expanded =
			expand_direct_invocation_with_config(vec![OsString::from("/tmp/if")], "if", &config)?;
		assert_eq!(
			expanded,
			["biwa", "run", "'if'"].map(OsString::from).to_vec()
		);
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_creates_updates_and_removes_managed_symlinks() -> Result<()> {
		use std::os::unix::fs::symlink;

		let dir = tempdir()?;
		let bin_dir = dir.path().join("bin");
		let current_biwa = dir.path().join("current/biwa");
		let old_biwa = dir.path().join("old/biwa");
		fs::create_dir_all(&bin_dir)?;
		fs::create_dir_all(current_biwa.parent().expect("parent"))?;
		fs::create_dir_all(old_biwa.parent().expect("parent"))?;
		fs::write(&current_biwa, "")?;
		fs::write(&old_biwa, "")?;
		symlink(&old_biwa, bin_dir.join("dcc"))?;
		symlink(&old_biwa, bin_dir.join("stale"))?;
		symlink("/usr/bin/true", bin_dir.join("unmanaged"))?;
		symlink(&old_biwa, bin_dir.join("unrelated-biwa"))?;
		fs::write(bin_dir.join(MANAGED_SHIMS_FILE), "dcc\nstale\n")?;

		let commands = BTreeMap::from([
			("1511".to_owned(), Vec::new()),
			("dcc".to_owned(), vec!["--skip-sync".to_owned()]),
		]);
		let config = direct_config(commands, bin_dir.clone());
		let report = install_shims(&config, &current_biwa, false)?;

		assert_eq!(
			report.installed,
			vec![bin_dir.join("1511"), bin_dir.join("dcc")]
		);
		assert_eq!(report.removed, vec![bin_dir.join("stale")]);
		assert_eq!(fs::read_link(bin_dir.join("dcc"))?, current_biwa);
		assert!(bin_dir.join("unmanaged").exists());
		assert!(bin_dir.join("unrelated-biwa").exists());
		assert_eq!(
			fs::read_to_string(bin_dir.join(MANAGED_SHIMS_FILE))?,
			"1511\ndcc\n"
		);
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_preserves_current_shim_and_requires_force_for_file() -> Result<()> {
		use std::os::unix::fs::symlink;

		let dir = tempdir()?;
		let bin_dir = dir.path().join("bin");
		let biwa = dir.path().join("biwa");
		fs::create_dir_all(&bin_dir)?;
		fs::write(&biwa, "")?;
		symlink(&biwa, bin_dir.join("dcc"))?;
		let config = direct_config(
			BTreeMap::from([("dcc".to_owned(), Vec::new())]),
			bin_dir.clone(),
		);

		let report = install_shims(&config, &biwa, false)?;
		assert_eq!(report.unchanged, vec![bin_dir.join("dcc")]);

		fs::remove_file(bin_dir.join("dcc"))?;
		fs::write(bin_dir.join("dcc"), "keep")?;
		let error =
			install_shims(&config, &biwa, false).expect_err("regular files should require --force");
		assert!(error.to_string().contains("Use `--force`"));
		install_shims(&config, &biwa, true)?;
		assert_eq!(fs::read_link(bin_dir.join("dcc"))?, biwa);
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_requires_force_to_replace_untracked_symlink() -> Result<()> {
		use std::os::unix::fs::symlink;

		let dir = tempdir()?;
		let bin_dir = dir.path().join("bin");
		let biwa = dir.path().join("biwa");
		let unrelated = dir.path().join("unrelated");
		fs::create_dir_all(&bin_dir)?;
		fs::write(&biwa, "")?;
		fs::write(&unrelated, "")?;
		symlink(&unrelated, bin_dir.join("dcc"))?;
		let config = direct_config(
			BTreeMap::from([("dcc".to_owned(), Vec::new())]),
			bin_dir.clone(),
		);

		let error = install_shims(&config, &biwa, false)
			.expect_err("untracked symlinks should require --force");
		assert!(error.to_string().contains("untracked symlink"));
		assert_eq!(fs::read_link(bin_dir.join("dcc"))?, unrelated);

		install_shims(&config, &biwa, true)?;
		assert_eq!(fs::read_link(bin_dir.join("dcc"))?, biwa);
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_rejects_unsafe_shim_directory() -> Result<()> {
		use std::os::unix::fs::PermissionsExt as _;

		let dir = tempdir()?;
		let bin_dir = dir.path().join("bin");
		let biwa = dir.path().join("biwa");
		fs::create_dir_all(&bin_dir)?;
		fs::set_permissions(&bin_dir, fs::Permissions::from_mode(0o770))?;
		fs::write(&biwa, "")?;
		let config = direct_config(BTreeMap::from([("dcc".to_owned(), Vec::new())]), bin_dir);

		let error = install_shims(&config, &biwa, false)
			.expect_err("group-writable PATH directory should fail");
		assert!(error.to_string().contains("group- or world-writable"));
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn activation_rejects_non_utf8_shim_directory() {
		use std::os::unix::ffi::OsStringExt as _;

		let path = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]));
		let error = activation_script(ActivationShell::Bash, &path)
			.expect_err("non-UTF-8 shell paths should fail explicitly");
		assert!(error.to_string().contains("not valid UTF-8"));
	}
}
