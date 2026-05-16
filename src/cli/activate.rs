use crate::Result;
use crate::cli::run::{RemoteCommand, run_remote};
use crate::cli::sync::SyncArgs;
use crate::config::types::Config;
use alloc::collections::BTreeSet;
use clap::{Args, Subcommand, ValueEnum};
use color_eyre::eyre::{WrapErr as _, bail};
use regex::Regex;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

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
	/// Create or update configured static command shims.
	Install(Install),
	/// Print diagnostic information for direct command activation.
	Doctor,
}

/// Direct command shim installation options.
#[derive(Args, Debug)]
struct Install {
	/// Replace existing shim files and ignore local command conflicts.
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

/// Direct command invocation extracted from argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectInvocation {
	/// Command name from `argv[0]`.
	command: String,
	/// User-supplied command arguments.
	args: Vec<String>,
}

/// Result from a shim installation run.
#[derive(Debug, Default, PartialEq, Eq)]
struct InstallReport {
	/// Newly installed shim paths.
	installed: Vec<PathBuf>,
	/// Existing shim paths that already pointed at the current executable.
	unchanged: Vec<PathBuf>,
	/// Commands skipped because an earlier local command exists.
	skipped_conflicts: Vec<ShimConflict>,
}

/// A local command conflict detected before shim installation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShimConflict {
	/// Command name.
	command: String,
	/// Earlier executable found on PATH.
	path: PathBuf,
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
	/// Run the activation command.
	pub(super) fn run(self) -> Result<()> {
		let config = Config::load_optional_ssh()?;
		let mut did_work = false;

		match self.command {
			Some(ActivateCommand::Install(install)) => {
				let path = env::var_os("PATH");
				let report =
					install_shims(&config, &env::current_exe()?, path.as_ref(), install.force)?;
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
			println!(
				"{}",
				activation_script(shell, &config.direct.bin_dir, config.direct.prefer_local)
			);
			did_work = true;
		}

		if !did_work {
			bail!("Specify `--shell <bash|zsh|fish>`, `install`, or `doctor`.");
		}

		Ok(())
	}
}

/// Returns a direct invocation when `argv[0]` is not the normal `biwa` binary name.
pub(super) fn direct_invocation_from_env() -> Result<Option<DirectInvocation>> {
	direct_invocation_from_args(env::args_os())
}

/// Runs a remote command through direct shim dispatch.
pub(super) async fn run_direct_invocation(
	invocation: DirectInvocation,
	quiet: bool,
	silent: bool,
) -> Result<()> {
	let (config, required_presence) = Config::load_optional_ssh_with_presence()?;

	if !config.direct.enabled {
		bail!(
			"Direct command `{}` is disabled. Set `direct.enabled = true` to use activation shims.",
			invocation.command
		);
	}

	let allow_patterns = compile_direct_allow_patterns(&config)?;
	if !direct_command_is_allowed(&allow_patterns, &invocation.command) {
		bail!(
			"Direct command `{}` is not allowed by `direct.allow`.",
			invocation.command
		);
	}

	let mut command_args = config
		.direct
		.default_args
		.get(&invocation.command)
		.cloned()
		.unwrap_or_default();
	command_args.extend(invocation.args);

	required_presence.ensure_all_present()?;
	run_remote(
		&config,
		&SyncArgs::default(),
		RemoteCommand {
			command: &invocation.command,
			command_args: &command_args,
			cli_env_vars: &[],
		},
		config.sync.auto,
		quiet,
		silent,
	)
	.await
}

/// Extracts a direct invocation from a supplied argument iterator.
fn direct_invocation_from_args(
	args: impl IntoIterator<Item = OsString>,
) -> Result<Option<DirectInvocation>> {
	let mut args = args.into_iter();
	let Some(argv0) = args.next() else {
		return Ok(None);
	};

	let Some(command) = direct_command_name(&argv0) else {
		return Ok(None);
	};

	let args = args
		.map(os_string_to_string)
		.collect::<Result<Vec<String>>>()?;

	Ok(Some(DirectInvocation { command, args }))
}

/// Returns the shim command name from `argv[0]`, excluding normal biwa invocations.
fn direct_command_name(argv0: &OsStr) -> Option<String> {
	let name = Path::new(argv0).file_name()?.to_str()?;
	if matches!(name, "biwa" | "biwa.exe") {
		return None;
	}
	Some(name.to_owned())
}

/// Converts an OS string argument into UTF-8 for remote execution.
fn os_string_to_string(value: OsString) -> Result<String> {
	value
		.into_string()
		.map_err(|value| color_eyre::eyre::eyre!("Non-UTF-8 direct command argument: {value:?}"))
}

/// Compiles configured direct command allow patterns.
fn compile_direct_allow_patterns(config: &Config) -> Result<Vec<Regex>> {
	config
		.direct
		.allow
		.iter()
		.map(|pattern| {
			Regex::new(pattern).wrap_err_with(|| format!("Invalid direct.allow regex `{pattern}`"))
		})
		.collect()
}

/// Returns whether a direct command name matches any configured allow pattern.
fn direct_command_is_allowed(allow_patterns: &[Regex], command: &str) -> bool {
	allow_patterns
		.iter()
		.any(|pattern| pattern.is_match(command))
}

/// Returns shell code that adds the shim directory to PATH.
fn activation_script(shell: ActivationShell, bin_dir: &Path, prefer_local: bool) -> String {
	let bin_dir = bin_dir.to_string_lossy();
	let quoted_bin_dir = shell_words::quote(&bin_dir);

	match shell {
		ActivationShell::Bash | ActivationShell::Zsh => {
			let export = if prefer_local {
				r#"if [ -n "$PATH" ]; then
  export PATH="$PATH:$__biwa_direct_bin"
else
  export PATH="$__biwa_direct_bin"
fi"#
			} else {
				r#"if [ -n "$PATH" ]; then
  export PATH="$__biwa_direct_bin:$PATH"
else
  export PATH="$__biwa_direct_bin"
fi"#
			};
			format!(
				r#"__biwa_direct_bin={quoted_bin_dir}
case ":$PATH:" in
  *:"$__biwa_direct_bin":*) ;;
  *) {export} ;;
esac
unset __biwa_direct_bin"#
			)
		}
		ActivationShell::Fish => {
			let set_path = if prefer_local {
				"set -gx PATH $PATH $__biwa_direct_bin"
			} else {
				"set -gx PATH $__biwa_direct_bin $PATH"
			};
			format!(
				"set -l __biwa_direct_bin {quoted_bin_dir}
if not contains -- $__biwa_direct_bin $PATH
  {set_path}
end
set -e __biwa_direct_bin"
			)
		}
	}
}

/// Creates or updates shims for statically known allowed command names.
fn install_shims(
	config: &Config,
	biwa_path: &Path,
	path: Option<&OsString>,
	force: bool,
) -> Result<InstallReport> {
	let mut report = InstallReport::default();
	let shim_names = static_allowed_shim_names(config)?;

	fs::create_dir_all(&config.direct.bin_dir).wrap_err_with(|| {
		format!(
			"Failed to create direct shim directory `{}`",
			config.direct.bin_dir.display()
		)
	})?;

	for command in shim_names {
		if !force
			&& config.direct.prefer_local
			&& let Some(conflict) = find_local_conflict(&command, &config.direct.bin_dir, path)
		{
			report.skipped_conflicts.push(conflict);
			continue;
		}

		let shim_path = config.direct.bin_dir.join(&command);
		match create_or_update_symlink(&shim_path, biwa_path, force)? {
			ShimInstallStatus::Installed => report.installed.push(shim_path),
			ShimInstallStatus::Unchanged => report.unchanged.push(shim_path),
		}
	}

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
	for conflict in &report.skipped_conflicts {
		eprintln!(
			"Skipped `{}` because `{}` appears earlier in PATH",
			conflict.command,
			conflict.path.display()
		);
	}
	if report.installed.is_empty()
		&& report.unchanged.is_empty()
		&& report.skipped_conflicts.is_empty()
	{
		eprintln!(
			"No static direct command shims to install. Add literal `direct.allow` entries or `direct.default_args` keys."
		);
	}
}

/// Prints direct command diagnostics.
fn print_doctor(config: &Config) -> Result<()> {
	let shim_names = static_allowed_shim_names(config)?;

	println!("direct.enabled = {}", config.direct.enabled);
	println!("direct.bin_dir = {}", config.direct.bin_dir.display());
	println!("direct.prefer_local = {}", config.direct.prefer_local);
	println!("static shims = {}", shim_names.len());
	for command in shim_names {
		println!("- {command}");
	}

	Ok(())
}

/// Returns static shim names that can be safely materialized.
fn static_allowed_shim_names(config: &Config) -> Result<Vec<String>> {
	let mut names = BTreeSet::new();
	let allow_patterns = compile_direct_allow_patterns(config)?;

	for pattern in &config.direct.allow {
		names.extend(static_names_from_allow_pattern(pattern));
	}

	names.extend(config.direct.default_args.keys().cloned());

	Ok(names
		.into_iter()
		.filter(|name| direct_command_is_allowed(&allow_patterns, name))
		.collect())
}

/// Extracts literal command names from a restricted subset of anchored regexes.
fn static_names_from_allow_pattern(pattern: &str) -> Vec<String> {
	let Some(inner) = pattern.strip_prefix('^').and_then(|s| s.strip_suffix('$')) else {
		return Vec::new();
	};

	let alternatives = inner
		.strip_prefix('(')
		.and_then(|s| s.strip_suffix(')'))
		.map_or_else(|| vec![inner.to_owned()], split_regex_group_alternatives);

	alternatives
		.iter()
		.filter_map(|alternative| static_name_from_regex_literal(alternative))
		.collect()
}

/// Splits a simple regex group on unescaped alternation separators.
fn split_regex_group_alternatives(group: &str) -> Vec<String> {
	let mut alternatives = Vec::new();
	let mut current = String::new();
	let mut escaped = false;

	for ch in group.chars() {
		if escaped {
			current.push('\\');
			current.push(ch);
			escaped = false;
		} else if ch == '\\' {
			escaped = true;
		} else if ch == '|' {
			alternatives.push(current);
			current = String::new();
		} else {
			current.push(ch);
		}
	}

	if escaped {
		current.push('\\');
	}
	alternatives.push(current);
	alternatives
}

/// Converts one simple regex literal into a command name.
fn static_name_from_regex_literal(literal: &str) -> Option<String> {
	let mut out = String::new();
	let mut chars = literal.chars();

	while let Some(ch) = chars.next() {
		if ch == '\\' {
			let escaped = chars.next()?;
			if regex_metachar(escaped) {
				out.push(escaped);
			} else {
				return None;
			}
		} else if regex_metachar(ch) {
			return None;
		} else {
			out.push(ch);
		}
	}

	(!out.is_empty()).then_some(out)
}

/// Returns whether a character has special meaning in regex syntax.
const fn regex_metachar(ch: char) -> bool {
	matches!(
		ch,
		'.' | '[' | ']' | '{' | '}' | '(' | ')' | '*' | '+' | '?' | '^' | '$' | '|' | '\\'
	)
}

/// Finds an earlier PATH command that should be preferred over a biwa shim.
fn find_local_conflict(
	command: &str,
	bin_dir: &Path,
	path: Option<&OsString>,
) -> Option<ShimConflict> {
	let path = path?;
	for entry in env::split_paths(path) {
		if same_path(&entry, bin_dir) {
			return None;
		}

		let candidate = entry.join(command);
		if is_executable_file(&candidate) {
			return Some(ShimConflict {
				command: command.to_owned(),
				path: candidate,
			});
		}
	}

	None
}

/// Returns whether two paths identify the same directory.
fn same_path(left: &Path, right: &Path) -> bool {
	if left == right {
		return true;
	}

	match (left.canonicalize(), right.canonicalize()) {
		(Ok(left), Ok(right)) => left == right,
		_ => false,
	}
}

/// Returns whether a path is an executable file.
#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
	use std::os::unix::fs::PermissionsExt as _;

	path.metadata()
		.is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Returns whether a path is an executable file.
#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
	path.metadata().is_ok_and(|metadata| metadata.is_file())
}

/// Creates or updates one symlink shim.
#[cfg(unix)]
fn create_or_update_symlink(
	shim_path: &Path,
	biwa_path: &Path,
	force: bool,
) -> Result<ShimInstallStatus> {
	use std::os::unix::fs::symlink;

	match fs::symlink_metadata(shim_path) {
		Ok(metadata) if metadata.file_type().is_symlink() => {
			if fs::read_link(shim_path).is_ok_and(|target| target == biwa_path) {
				return Ok(ShimInstallStatus::Unchanged);
			}
			fs::remove_file(shim_path).wrap_err_with(|| {
				format!(
					"Failed to replace direct command shim `{}`",
					shim_path.display()
				)
			})?;
		}
		Ok(_) => {
			if force {
				if shim_path.is_dir() {
					bail!(
						"Refusing to replace existing directory `{}`",
						shim_path.display()
					);
				}
				fs::remove_file(shim_path).wrap_err_with(|| {
					format!(
						"Failed to replace direct command shim `{}`",
						shim_path.display()
					)
				})?;
			} else {
				bail!(
					"Refusing to replace existing non-symlink `{}`. Use `--force` to replace it.",
					shim_path.display()
				);
			}
		}
		Err(error) if error.kind() == ErrorKind::NotFound => {}
		Err(error) => {
			return Err(error).wrap_err_with(|| {
				format!(
					"Failed to inspect direct command shim `{}`",
					shim_path.display()
				)
			});
		}
	}

	symlink(biwa_path, shim_path).wrap_err_with(|| {
		format!(
			"Failed to create direct command shim `{}` -> `{}`",
			shim_path.display(),
			biwa_path.display()
		)
	})?;

	Ok(ShimInstallStatus::Installed)
}

/// Creates or updates one symlink shim.
#[cfg(not(unix))]
fn create_or_update_symlink(
	_shim_path: &Path,
	_biwa_path: &Path,
	_force: bool,
) -> Result<ShimInstallStatus> {
	bail!("Direct command shim installation is only supported on Unix-like systems");
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::types::Config;
	use alloc::collections::BTreeMap;
	use pretty_assertions::assert_eq;
	use std::ffi::OsString;
	use std::fs;
	use tempfile::tempdir;

	fn direct_config(
		allow: Vec<String>,
		default_args: BTreeMap<String, Vec<String>>,
		prefer_local: bool,
	) -> Config {
		let mut config = Config::default();
		config.direct.enabled = true;
		config.direct.allow = allow;
		config.direct.default_args = default_args;
		config.direct.prefer_local = prefer_local;
		config
	}

	#[test]
	fn direct_invocation_detects_non_biwa_argv0() -> Result<()> {
		let invocation = direct_invocation_from_args([
			OsString::from("/tmp/1511"),
			OsString::from("autotest"),
			OsString::from("lab01"),
		])?
		.expect("direct invocation should be detected");

		assert_eq!(
			invocation,
			DirectInvocation {
				command: "1511".to_owned(),
				args: vec!["autotest".to_owned(), "lab01".to_owned()],
			}
		);
		Ok(())
	}

	#[test]
	fn direct_invocation_ignores_normal_biwa_argv0() -> Result<()> {
		let invocation = direct_invocation_from_args([
			OsString::from("/usr/bin/biwa"),
			OsString::from("run"),
			OsString::from("pwd"),
		])?;

		assert_eq!(invocation, None);
		Ok(())
	}

	#[test]
	fn allow_patterns_match_command_names() -> Result<()> {
		let config = direct_config(
			vec![
				"^\\d{4}$".to_owned(),
				"^(give|autotest|dcc|1521)$".to_owned(),
			],
			BTreeMap::new(),
			true,
		);

		let allow_patterns = compile_direct_allow_patterns(&config)?;

		assert!(direct_command_is_allowed(&allow_patterns, "1511"));
		assert!(direct_command_is_allowed(&allow_patterns, "dcc"));
		assert!(!direct_command_is_allowed(&allow_patterns, "1511x"));
		assert!(!direct_command_is_allowed(&allow_patterns, "sh"));
		Ok(())
	}

	#[test]
	fn static_shim_names_come_from_literals_and_default_args() -> Result<()> {
		let mut default_args = BTreeMap::new();
		default_args.insert(
			"1511".to_owned(),
			vec!["--course".to_owned(), "comp1511".to_owned()],
		);
		default_args.insert("9999".to_owned(), vec!["ignored".to_owned()]);
		let config = direct_config(
			vec![
				"^\\d{4}$".to_owned(),
				"^(give|autotest|dcc|1521)$".to_owned(),
			],
			default_args,
			true,
		);

		assert_eq!(
			static_allowed_shim_names(&config)?,
			vec!["1511", "1521", "9999", "autotest", "dcc", "give"]
		);
		Ok(())
	}

	#[test]
	fn static_shim_names_preserve_escaped_pipe_in_alternatives() -> Result<()> {
		let config = direct_config(
			vec!["^(cmd1|cmd\\|2|dcc)$".to_owned()],
			BTreeMap::new(),
			true,
		);

		assert_eq!(
			static_allowed_shim_names(&config)?,
			vec!["cmd1", "cmd|2", "dcc"]
		);
		Ok(())
	}

	#[test]
	fn activation_output_appends_path_when_preferring_local() {
		let script = activation_script(ActivationShell::Bash, Path::new("/tmp/biwa/bin"), true);

		assert!(script.contains(r#"export PATH="$PATH:$__biwa_direct_bin""#));
		assert!(script.contains(r#"export PATH="$__biwa_direct_bin""#));
		assert!(script.contains("/tmp/biwa/bin"));
	}

	#[test]
	fn activation_output_prepends_path_when_not_preferring_local() {
		let script = activation_script(ActivationShell::Fish, Path::new("/tmp/biwa/bin"), false);

		assert!(script.contains("set -gx PATH $__biwa_direct_bin $PATH"));
		assert!(script.contains("/tmp/biwa/bin"));
	}

	#[test]
	fn direct_default_args_are_inserted_before_user_args() {
		let mut default_args = BTreeMap::new();
		default_args.insert(
			"1511".to_owned(),
			vec!["--course".to_owned(), "comp1511".to_owned()],
		);
		let invocation = DirectInvocation {
			command: "1511".to_owned(),
			args: vec!["autotest".to_owned(), "lab01".to_owned()],
		};
		let mut args = default_args
			.get(&invocation.command)
			.cloned()
			.unwrap_or_default();
		args.extend(invocation.args);

		assert_eq!(args, vec!["--course", "comp1511", "autotest", "lab01"]);
	}

	#[test]
	fn local_conflict_detects_command_before_shim_dir() -> Result<()> {
		let dir = tempdir()?;
		let local_bin = dir.path().join("local");
		let shim_bin = dir.path().join("shim");
		fs::create_dir_all(&local_bin)?;
		fs::create_dir_all(&shim_bin)?;
		let command = local_bin.join("dcc");
		fs::write(&command, "#!/bin/sh\n")?;

		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt as _;
			fs::set_permissions(&command, fs::Permissions::from_mode(0o755))?;
		}

		let path = env::join_paths([local_bin.as_path(), shim_bin.as_path()])?;

		assert_eq!(
			find_local_conflict("dcc", &shim_bin, Some(&path)),
			Some(ShimConflict {
				command: "dcc".to_owned(),
				path: command,
			})
		);
		Ok(())
	}

	#[test]
	fn install_skips_conflicts_when_prefer_local_is_enabled() -> Result<()> {
		let dir = tempdir()?;
		let local_bin = dir.path().join("local");
		let shim_bin = dir.path().join("shim");
		let biwa = dir.path().join("biwa");
		fs::create_dir_all(&local_bin)?;
		fs::write(&biwa, "")?;
		let command = local_bin.join("dcc");
		fs::write(&command, "#!/bin/sh\n")?;

		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt as _;
			fs::set_permissions(&command, fs::Permissions::from_mode(0o755))?;
		}

		let mut config = direct_config(vec!["^dcc$".to_owned()], BTreeMap::new(), true);
		config.direct.bin_dir = shim_bin.clone();
		let path = env::join_paths([local_bin.as_path()])?;

		let report = install_shims(&config, &biwa, Some(&path), false)?;

		assert!(report.installed.is_empty());
		assert_eq!(
			report.skipped_conflicts,
			vec![ShimConflict {
				command: "dcc".to_owned(),
				path: command,
			}]
		);
		assert!(!shim_bin.join("dcc").exists());
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_force_ignores_conflicts_when_prefer_local_is_enabled() -> Result<()> {
		use std::os::unix::fs::PermissionsExt as _;

		let dir = tempdir()?;
		let local_bin = dir.path().join("local");
		let shim_bin = dir.path().join("shim");
		let biwa = dir.path().join("biwa");
		fs::create_dir_all(&local_bin)?;
		fs::write(&biwa, "")?;
		let command = local_bin.join("dcc");
		fs::write(&command, "#!/bin/sh\n")?;

		fs::set_permissions(&command, fs::Permissions::from_mode(0o755))?;

		let mut config = direct_config(vec!["^dcc$".to_owned()], BTreeMap::new(), true);
		config.direct.bin_dir = shim_bin.clone();
		let path = env::join_paths([local_bin.as_path()])?;

		let report = install_shims(&config, &biwa, Some(&path), true)?;

		assert_eq!(report.installed, vec![shim_bin.join("dcc")]);
		assert!(report.skipped_conflicts.is_empty());
		assert_eq!(fs::read_link(shim_bin.join("dcc"))?, biwa);
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_creates_symlink_for_static_allowed_command() -> Result<()> {
		let dir = tempdir()?;
		let shim_bin = dir.path().join("shim");
		let biwa = dir.path().join("biwa");
		fs::write(&biwa, "")?;

		let mut config = direct_config(vec!["^dcc$".to_owned()], BTreeMap::new(), true);
		config.direct.bin_dir = shim_bin.clone();

		let report = install_shims(&config, &biwa, None, false)?;

		assert_eq!(report.installed, vec![shim_bin.join("dcc")]);
		assert_eq!(fs::read_link(shim_bin.join("dcc"))?, biwa);
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn install_force_replaces_existing_file() -> Result<()> {
		let dir = tempdir()?;
		let shim_bin = dir.path().join("shim");
		let biwa = dir.path().join("biwa");
		fs::create_dir_all(&shim_bin)?;
		fs::write(&biwa, "")?;
		fs::write(shim_bin.join("dcc"), "#!/bin/sh\n")?;

		let mut config = direct_config(vec!["^dcc$".to_owned()], BTreeMap::new(), true);
		config.direct.bin_dir = shim_bin.clone();

		let report = install_shims(&config, &biwa, None, true)?;

		assert_eq!(report.installed, vec![shim_bin.join("dcc")]);
		assert_eq!(fs::read_link(shim_bin.join("dcc"))?, biwa);
		Ok(())
	}
}
