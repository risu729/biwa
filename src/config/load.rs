use super::format::ConfigFormat;
use super::types::Config;
use confique::Config as _;
use eyre::{Result, WrapErr};
use std::path::{Path, PathBuf};

impl Config {
	pub fn load() -> Result<Self> {
		let home = homedir::my_home().ok().flatten();
		let xdg = std::env::var("XDG_CONFIG_HOME").ok().map(PathBuf::from);
		let cwd = std::env::current_dir().ok();
		Self::load_internal(home.as_ref(), xdg.as_ref(), cwd.as_ref())
	}

	fn load_internal(
		home: Option<&PathBuf>,
		xdg: Option<&PathBuf>,
		cwd: Option<&PathBuf>,
	) -> Result<Self> {
		let mut builder = Self::builder().env();

		let mut global_candidates = Vec::new();
		let mut global_root: Option<PathBuf> = None;
		if let Some(home_path) = home {
			global_candidates.push(home_path.join("biwa"));
			global_candidates.push(home_path.join(".biwa"));
			let config_home = xdg.cloned().unwrap_or_else(|| home_path.join(".config"));
			global_candidates.push(config_home.join("biwa/config"));
			// All global configs should resolve relative paths from the home dir (~)
			global_root = Some(home_path.clone());
		}

		if let Some(cwd_path) = cwd {
			let mut current = Some(cwd_path.as_path());

			let mut layers = Vec::new();
			while let Some(path) = current {
				if let Some(home_path) = home
					&& path == home_path
				{
					break;
				}

				if path.parent().is_none() {
					break;
				}

				layers.push(path.to_path_buf());
				current = path.parent();
			}

			// Higher precedence sources must be added first in confique!
			// So iterate layers from cwd (innermost) up to outer directory.
			for path in &layers {
				// Don't load config from .config directory itself
				if path.file_name().and_then(|s| s.to_str()) == Some(".config") {
					continue;
				}

				let local_candidates = vec![
					path.join("biwa"),
					path.join(".biwa"),
					path.join(".config/biwa"),
				];

				// For local configs, the config root is always the project root
				// represented by this layer (`path`):
				// - `biwa.toml` / `.biwa.toml`  -> `.`
				// - `.config/biwa.toml`        -> `..`
				let config_root = path;

				if let Some((config_path, format)) = find_single_config(&local_candidates)? {
					let partial = Self::load_partial(&config_path, format, config_root)?;
					builder = builder.preloaded(partial);
				}
			}
		}

		if let Some((config_path, format)) = find_single_config(&global_candidates)? {
			// Global configs should resolve relative paths from the home directory (~),
			// regardless of where the config file actually lives (e.g. XDG paths).
			let config_root = global_root
				.as_deref()
				.unwrap_or_else(|| config_path.parent().unwrap_or_else(|| Path::new("")));
			let partial = Self::load_partial(&config_path, format, config_root)?;
			builder = builder.preloaded(partial);
		}

		let config = builder.load()?;

		Ok(config)
	}

	fn load_partial(
		path: &Path,
		format: ConfigFormat,
		config_root: &Path,
	) -> Result<<Self as confique::Config>::Partial> {
		let content = std::fs::read_to_string(path).wrap_err("Failed to read config file")?;
		let mut partial: <Self as confique::Config>::Partial = match format {
			ConfigFormat::Toml => toml::from_str(&content).wrap_err("Failed to parse TOML")?,
			ConfigFormat::Yaml => {
				serde_yaml::from_str(&content).wrap_err("Failed to parse YAML")?
			}
			ConfigFormat::Json | ConfigFormat::Json5 => {
				json5::from_str(&content).wrap_err("Failed to parse JSON")?
			}
		};

		Self::resolve_paths_partial(&mut partial, config_root);

		Ok(partial)
	}

	fn resolve_paths_partial(partial: &mut <Self as confique::Config>::Partial, root: &Path) {
		let resolve = |path_opt: &mut Option<PathBuf>| {
			if let Some(path) = path_opt {
				*path = expand_tilde(path);
				if path.is_relative() {
					*path = root.join(&*path);
				}
			}
		};

		resolve(&mut partial.ssh.key_path);
		resolve(&mut partial.sync.remote_root);
	}

	pub fn template(format: ConfigFormat) -> String {
		match format {
			ConfigFormat::Toml => {
				confique::toml::template::<Self>(confique::toml::FormatOptions::default())
			}
			ConfigFormat::Yaml => {
				confique::yaml::template::<Self>(confique::yaml::FormatOptions::default())
			}
			ConfigFormat::Json | ConfigFormat::Json5 => {
				confique::json5::template::<Self>(confique::json5::FormatOptions::default())
			}
		}
	}
}

fn expand_tilde(path: &Path) -> PathBuf {
	if let Some(home) = homedir::my_home().ok().flatten()
		&& let Some(s) = path.to_str()
	{
		if let Some(rest) = s.strip_prefix("~/") {
			return home.join(rest);
		}
		if s == "~" {
			return home;
		}
	}
	path.to_path_buf()
}

fn find_single_config(base_paths_no_ext: &[PathBuf]) -> Result<Option<(PathBuf, ConfigFormat)>> {
	let mut found = Vec::new();

	for base in base_paths_no_ext {
		for format in ConfigFormat::all() {
			for ext in format.extensions() {
				let path = base.with_extension(ext);
				if path.exists() {
					found.push((path, *format));
				}
			}
		}
	}

	if found.len() > 1 {
		found.sort_by(|(a, _), (b, _)| a.cmp(b));
		found.dedup_by(|(a, _), (b, _)| a == b);

		if found.len() > 1 {
			let paths: Vec<String> = found
				.iter()
				.map(|(p, _)| p.to_string_lossy().into_owned())
				.collect();
			eyre::bail!(
				"Multiple configuration files found in the same scope: {}. Only one is allowed.",
				paths.join(", ")
			);
		}
	}

	Ok(found.into_iter().next())
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use rstest::rstest;
	use std::fs;
	use std::sync::Mutex;
	use tempfile::tempdir;

	static TEST_MUTEX: Mutex<()> = Mutex::new(());

	#[test]
	fn test_default() {
		let config = Config::default();
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
		assert_eq!(config.ssh.port, 22);
		assert_eq!(config.ssh.user, "z1234567");
		assert!(config.sync.remote_root.ends_with(".cache/biwa/projects"));
	}

	#[test]
	fn test_env_override() {
		let _guard = TEST_MUTEX.lock().unwrap();
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "file""#).unwrap();

		// Set env var override
		unsafe {
			std::env::set_var("BIWA_SSH_HOST", "env");
			std::env::set_var("BIWA_SSH_PORT", "8080");
		}

		// Ensure cleanup
		let _cleanup1 = EnvCleanup("BIWA_SSH_HOST");
		let _cleanup2 = EnvCleanup("BIWA_SSH_PORT");

		let config = Config::load_internal(
			Some(&dir.path().to_path_buf()),
			None,
			Some(&dir.path().to_path_buf()),
		)
		.unwrap();

		assert_eq!(config.ssh.host, "env"); // Env overrides file
		assert_eq!(config.ssh.port, 8080); // Env override works
	}

	struct EnvCleanup(&'static str);
	impl Drop for EnvCleanup {
		fn drop(&mut self) {
			unsafe {
				std::env::remove_var(self.0);
			}
		}
	}

	#[test]
	fn test_strict_global_config() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("biwa.toml"), "ssh.port = 2222").unwrap();

		let config = Config::load_internal(Some(&dir.path().to_path_buf()), None, None).unwrap();
		assert_eq!(config.ssh.port, 2222);

		// Now write a competing global config format
		fs::write(dir.path().join("biwa.json"), r#"{"ssh": {"port": 3333}}"#).unwrap();
		let result = Config::load_internal(Some(&dir.path().to_path_buf()), None, None);
		assert!(
			result.is_err(),
			"Expected error due to multiple global configs"
		);
	}

	#[test]
	fn test_strict_local_config() {
		let home = tempdir().unwrap();
		let cwd = tempdir().unwrap();

		fs::write(cwd.path().join("biwa.yaml"), "ssh:\n  port: 2222").unwrap();

		let config = Config::load_internal(
			Some(&home.path().to_path_buf()),
			None,
			Some(&cwd.path().to_path_buf()),
		)
		.unwrap();
		assert_eq!(config.ssh.port, 2222);

		// Now write a competing local config format
		fs::write(cwd.path().join("biwa.json"), r#"{"ssh": {"port": 3333}}"#).unwrap();
		let result = Config::load_internal(
			Some(&home.path().to_path_buf()),
			None,
			Some(&cwd.path().to_path_buf()),
		);
		assert!(
			result.is_err(),
			"Expected error due to multiple local configs"
		);
	}

	#[test]
	fn test_nested_within_dot_config() {
		let cwd = tempdir().unwrap();
		let dot_config = cwd.path().join(".config");
		fs::create_dir_all(&dot_config).unwrap();

		fs::write(cwd.path().join("biwa.toml"), "ssh.port = 2222").unwrap();
		fs::write(dot_config.join("biwa.toml"), "ssh.port = 3333").unwrap();

		// If a hierarchy exists like .config/biwa.toml alongside biwa.toml
		// .config/biwa.toml has HIGHER precedence initially since it's deeper,
		// but find_single_config checks them sequentially in scope order.
		let result = Config::load_internal(None, None, Some(&cwd.path().to_path_buf()));
		assert!(
			result.is_err(),
			"Expected error because biwa.toml and .config/biwa.toml are both valid base paths in the same workspace local scope."
		);
	}

	#[rstest]
	#[case(true)]
	#[case(false)]
	fn test_xdg_precedence(#[case] use_custom_xdg: bool) {
		let home = tempdir().unwrap();
		let custom_xdg = tempdir().unwrap();

		// Put identical biwa/config files in both ~/.config and custom XDG
		let default_config_dir = home.path().join(".config").join("biwa");
		fs::create_dir_all(&default_config_dir).unwrap();
		fs::write(default_config_dir.join("config.toml"), "ssh.port = 2222").unwrap();

		let custom_config_dir = custom_xdg.path().join("biwa");
		fs::create_dir_all(&custom_config_dir).unwrap();
		fs::write(custom_config_dir.join("config.toml"), "ssh.port = 3333").unwrap();

		let xdg_path = if use_custom_xdg {
			Some(custom_xdg.path().to_path_buf())
		} else {
			None
		};

		let config =
			Config::load_internal(Some(&home.path().to_path_buf()), xdg_path.as_ref(), None)
				.unwrap();

		if use_custom_xdg {
			assert_eq!(config.ssh.port, 3333);
		} else {
			assert_eq!(config.ssh.port, 2222);
		}
	}

	#[test]
	fn test_traversal_precedence() {
		let home = tempdir().unwrap();
		let outer = tempdir().unwrap();
		let inner = outer.path().join("inner");
		fs::create_dir_all(&inner).unwrap();

		fs::write(outer.path().join("biwa.toml"), "ssh.port = 2222").unwrap();
		fs::write(inner.join("biwa.toml"), "ssh.port = 3333").unwrap();

		let config =
			Config::load_internal(Some(&home.path().to_path_buf()), None, Some(&inner.clone()))
				.unwrap();
		assert_eq!(
			config.ssh.port, 3333,
			"Deeper config should override shallower"
		);
	}

	#[test]
	fn test_traversal_stops_at_home() {
		// Mock home dir that is inside another tempdir
		let super_home = tempdir().unwrap();
		let home = super_home.path().join("user");
		fs::create_dir_all(&home).unwrap();

		// Config outside home SHOULD NOT be loaded
		fs::write(super_home.path().join("biwa.toml"), "ssh.port = 2222").unwrap();

		let config = Config::load_internal(Some(&home.clone()), None, Some(&home.clone())).unwrap();
		assert_eq!(
			config.ssh.port, 22,
			"Config outside home directory should be ignored"
		);
	}

	#[test]
	fn test_local_dot_config_support() {
		let home = tempdir().unwrap();
		let cwd = tempdir().unwrap();
		let dot_config = cwd.path().join(".config");
		fs::create_dir(&dot_config).unwrap();

		// Make sure it loads biwa.toml from .config/biwa.toml
		fs::write(dot_config.join("biwa.toml"), "ssh.port = 4444").unwrap();

		let config = Config::load_internal(
			Some(&home.path().to_path_buf()),
			None,
			Some(&cwd.path().to_path_buf()),
		)
		.unwrap();
		assert_eq!(
			config.ssh.port, 4444,
			"Should load `.config/biwa.toml` as a valid local config candidate"
		);
	}

	#[test]
	fn test_multiple_configs_error() {
		let root = tempdir().unwrap();
		let root = root.path();

		// 1. Multiple identical extensions -> OS/Cargo handles file overwriting,
		//    but if we have .toml and .toml just test uniqueness by base.
		//    Actually we just test diff extensions.

		// 2. Diff extensions same base -> Error
		fs::write(root.join("biwa.toml"), "").unwrap();
		fs::write(root.join("biwa.yaml"), "").unwrap();

		let result = find_single_config(&[root.join("biwa")]);
		assert!(result.is_err());

		// 3. Multiple formats for same base -> Error (Strictness)
		// Note: find_single_config logic checks across extensions for a single base too?
		// "Multiple configuration files found in the same scope"
		fs::write(root.join("biwa.json"), "{}").unwrap();
		let result = find_single_config(&[root.join("biwa")]);
		assert!(result.is_err());

		// Cleanup for next check
		fs::remove_file(root.join("biwa.toml")).unwrap();
		fs::remove_file(root.join("biwa.json")).unwrap();

		// 4. Multiple bases in list -> Error if both exist
		// e.g. biwa.toml and .biwa.toml
		fs::write(root.join("biwa.toml"), "").unwrap();
		fs::write(root.join(".biwa.toml"), "").unwrap();
		let result = find_single_config(&[root.join("biwa"), root.join(".biwa")]);
		assert!(result.is_err());
	}

	#[test]
	fn test_relative_key_path_resolved_against_source_config() {
		let _guard = TEST_MUTEX.lock().unwrap();
		// Layout:
		//   /parent/biwa.toml       -> sets ssh.key_path = "my_key"
		//   /parent/my_key          -> the key file
		//   /parent/child/biwa.toml -> overrides ssh.host only
		let parent = tempdir().unwrap();
		let child = parent.path().join("child");
		fs::create_dir_all(&child).unwrap();

		fs::write(
			parent.path().join("biwa.toml"),
			"[ssh]\nkey_path = \"my_key\"\n",
		)
		.unwrap();
		fs::write(parent.path().join("my_key"), "fake key").unwrap();
		fs::write(child.join("biwa.toml"), "[ssh]\nhost = \"other.host\"\n").unwrap();

		let config =
			Config::load_internal(None, None, Some(&child)).expect("failed to load config");

		// key_path should be resolved to parent/my_key, not child/my_key
		let resolved = config.ssh.key_path.expect("key_path should be set");
		let expected = parent.path().join("my_key");
		assert_eq!(resolved, expected);
	}

	#[test]
	fn test_nested_path_resolution() {
		let _guard = TEST_MUTEX.lock().unwrap();
		let dir = tempdir().unwrap();
		let root = dir.path();
		let subdir = root.join("subdir");
		fs::create_dir_all(&subdir).unwrap();

		// Parent config defines a relative path
		// "libs" should be resolved relative to `root`
		fs::write(
			root.join("biwa.toml"),
			r#"
[sync]
remote_root = "libs"
"#,
		)
		.unwrap();

		// Child config overrides something else, but inherits remote_root
		fs::write(
			subdir.join("biwa.toml"),
			r#"
[ssh]
host = "child"
"#,
		)
		.unwrap();

		// Load config from subdir
		// Expected: remote_root should be root/libs
		// Actual (bug): remote_root is subdir/libs because it resolves relative to the innermost config root
		let config =
			Config::load_internal(None, None, Some(&subdir)).expect("Failed to load config");

		let expected_path = root.join("libs");

		// This assertion will fail if the bug exists
		assert_eq!(
			config.sync.remote_root, expected_path,
			"remote_root should be resolved relative to the config file that defined it"
		);
	}

	#[test]
	fn test_local_config_root_dot_config_biwa() {
		let _guard = TEST_MUTEX.lock().unwrap();
		let dir = tempdir().unwrap();
		let project = dir.path().join("project");
		let dot_config = project.join(".config");
		fs::create_dir_all(&dot_config).unwrap();

		// Local config in .config/biwa.toml should use the project root (`project`)
		// as its config root, not the .config directory itself.
		fs::write(
			dot_config.join("biwa.toml"),
			r#"
[sync]
remote_root = "libs"
"#,
		)
		.unwrap();

		let config =
			Config::load_internal(None, None, Some(&project)).expect("Failed to load config");

		let expected_path = project.join("libs");
		assert_eq!(
			config.sync.remote_root, expected_path,
			"remote_root from .config/biwa.toml should be resolved relative to the project root"
		);
	}

	#[test]
	fn test_global_config_root_home_and_xdg() {
		let _guard = TEST_MUTEX.lock().unwrap();
		let dir = tempdir().unwrap();
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(&home).unwrap();
		fs::create_dir_all(config_home.join("biwa")).unwrap();

		// Global config at ~/biwa.toml
		fs::write(
			home.join("biwa.toml"),
			r#"
[sync]
remote_root = "global_libs"
"#,
		)
		.unwrap();

		let config = Config::load_internal(Some(&home), Some(&config_home), None)
			.expect("Failed to load config");
		assert_eq!(
			config.sync.remote_root,
			home.join("global_libs"),
			"global remote_root from ~/biwa.toml should be resolved relative to ~"
		);

		// Only one global config is allowed; remove the home config before testing the XDG variant.
		fs::remove_file(home.join("biwa.toml")).unwrap();

		// Override with XDG-style global config at ~/.config/biwa/config.toml
		fs::write(
			config_home.join("biwa/config.toml"),
			r#"
[sync]
remote_root = "xdg_libs"
"#,
		)
		.unwrap();

		let config = Config::load_internal(Some(&home), Some(&config_home), None)
			.expect("Failed to load config");
		assert_eq!(
			config.sync.remote_root,
			home.join("xdg_libs"),
			"global remote_root from ~/.config/biwa/config.toml should be resolved relative to ~"
		);
	}
}
