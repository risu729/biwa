use super::format::ConfigFormat;
use super::types::Config;
use confique::Config as _;
use eyre::{Result, WrapErr as _};
use std::path::{Path, PathBuf};
use std::{env, fs};

impl Config {
	/// Loads the configuration based on global, user, and project-local paths.
	pub fn load() -> Result<Self> {
		let home = homedir::my_home().ok().flatten();
		let xdg = env::var("XDG_CONFIG_HOME").ok().map(PathBuf::from);
		let cwd = env::current_dir().ok();
		Self::load_internal(home.as_ref(), xdg.as_ref(), cwd.as_ref())
	}

	/// Core inner load logic separating the paths.
	fn load_internal(
		home: Option<&PathBuf>,
		xdg: Option<&PathBuf>,
		cwd: Option<&PathBuf>,
	) -> Result<Self> {
		let mut builder = Self::builder().env();

		let mut global_candidates = Vec::new();
		let global_root: Option<PathBuf> = home.map(|home_path| {
			global_candidates.push(home_path.join("biwa"));
			global_candidates.push(home_path.join(".biwa"));
			let config_home = xdg.cloned().unwrap_or_else(|| home_path.join(".config"));
			global_candidates.push(config_home.join("biwa/config"));
			// All global configs should resolve relative paths from the home dir (~)
			home_path.clone()
		});

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

	/// Loads a specific partial configuration file based on format.
	fn load_partial(
		path: &Path,
		format: ConfigFormat,
		config_root: &Path,
	) -> Result<<Self as confique::Config>::Layer> {
		let content = fs::read_to_string(path).wrap_err("Failed to read config file")?;
		let mut partial: <Self as confique::Config>::Layer = match format {
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

	/// Resolves any relative paths within the configuration layer to be absolute based on the root path.
	fn resolve_paths_partial(partial: &mut <Self as confique::Config>::Layer, root: &Path) {
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

	/// Returns a string template of the default configuration for the specific format.
	#[expect(clippy::absolute_paths, reason = "use will be confusing here")]
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

/// Expands a tilde (`~`) at the start of a path to the user's home directory.
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

/// Tries to find exactly one config file from base path list. Errors on multiple files.
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
	use crate::testing::EnvCleanup;
	use assert_matches::assert_matches;
	use pretty_assertions::assert_eq;
	use rstest::rstest;
	use serial_test::serial;
	use std::fs;
	use tempfile::tempdir;

	#[serial]
	#[test]
	fn default() {
		let config = Config::default();
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
		assert_eq!(config.ssh.port, 22);
		assert_eq!(config.ssh.user, "z1234567");
		assert!(config.sync.remote_root.ends_with(".cache/biwa/projects"));
	}

	#[serial]
	#[test]
	fn env_override() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "file""#).unwrap();

		// Set env var override
		// SAFETY: This is a single-threaded test context modifying the environment for current process.
		// `#[serial]` from `serial_test` ensures serialized execution to prevent env races.
		unsafe {
			env::set_var("BIWA_SSH_HOST", "env");
		}
		// Set env var override
		// SAFETY: This is a single-threaded test context modifying the environment for current process.
		// `#[serial]` from `serial_test` ensures serialized execution to prevent env races.
		unsafe {
			env::set_var("BIWA_SSH_PORT", "8080");
		}

		// Ensure cleanup
		let _cleanup1 = EnvCleanup("BIWA_SSH_HOST");
		let _cleanup2 = EnvCleanup("BIWA_SSH_PORT");

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())
			.expect("Failed to load config");

		assert_eq!(config.ssh.host, "env");
		assert_eq!(config.ssh.port, 8080);
	}

	#[serial]
	#[test]
	fn snapshot() {
		let config = Config::default();
		insta::assert_json_snapshot!(config, @r#"
		{
		  "ssh": {
		    "host": "cse.unsw.edu.au",
		    "port": 22,
		    "user": "z1234567",
		    "key_path": null,
		    "password": false
		  },
		  "sync": {
		    "auto": true,
		    "remote_root": "~/.cache/biwa/projects",
		    "ignore_files": [
		      ".git",
		      "target",
		      "node_modules"
		    ],
		    "engine": "sftp",
		    "sftp": {
		      "max_files_to_sync": 100
		    }
		  },
		  "env": {
		    "vars": []
		  },
		  "hooks": {
		    "pre_sync": null,
		    "post_sync": null
		  },
		  "log": {
		    "quiet": false,
		    "silent": false
		  }
		}
		"#);
	}

	#[rstest]
	#[serial]
	#[case::toml("ssh.host = 'toml'", "toml", "toml")]
	#[case::json(r#"{ "ssh": { "host": "json" } }"#, "json", "json")]
	#[case::json5("{ ssh: { host: 'json5' } }", "json5", "json5")]
	#[case::yaml("ssh:\n  host: yaml", "yaml", "yaml")]
	fn format_extensions(#[case] content: &str, #[case] ext: &str, #[case] expected: &str) {
		let dir = tempdir().unwrap();
		let file_path = dir.path().join(format!("biwa.{ext}"));
		fs::write(&file_path, content).unwrap();

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, expected);
	}

	#[serial]
	#[test]
	fn traversal_precedence() {
		let dir = tempdir().unwrap();
		let root = dir.path();
		let subdir = root.join("subdir");
		let nested = subdir.join("nested");
		fs::create_dir_all(&nested).unwrap();

		fs::write(root.join("biwa.toml"), r#"ssh.host = "root""#).unwrap();
		fs::write(subdir.join("biwa.toml"), r#"ssh.host = "subdir""#).unwrap();

		let config = Config::load_internal(None, None, Some(nested).as_ref())
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, "subdir");
	}

	#[serial]
	#[test]
	fn traversal_stops_at_home() {
		let dir = tempdir().unwrap();
		let root = dir.path();
		let home = root.join("home");
		let project = home.join("project");
		fs::create_dir_all(&project).unwrap();

		// Config in root (parent of home) - should NOT be loaded if traversal stops at home
		fs::write(root.join("biwa.toml"), r#"ssh.host = "outside""#).unwrap();

		// We need to initialize the home dir so it's a valid path for test logic if needed
		fs::create_dir_all(&home).unwrap();

		let config = Config::load_internal(Some(&home), None, Some(&project))
			.expect("Failed to load config");

		assert_ne!(config.ssh.host, "outside");
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
	}

	#[serial]
	#[test]
	fn xdg_precedence() {
		let dir = tempdir().unwrap();
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa")).unwrap();

		fs::write(config_home.join("biwa/config.toml"), r#"ssh.host = "xdg""#).unwrap();

		let config = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None)
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, "xdg");
	}

	#[serial]
	#[test]
	fn cwd_is_dot_config() {
		let dir = tempdir().unwrap();
		let project = dir.path().join("project");
		let dot_config = project.join(".config");
		let biwa_dir = dot_config.join("biwa");
		fs::create_dir_all(&biwa_dir).unwrap();

		// Standard config locatable from 'project' layer
		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "standard""#).unwrap();

		// Weird config only locatable if '.config' is a layer
		fs::write(dot_config.join(".biwa.toml"), r#"ssh.host = "weird""#).unwrap();

		// CWD is .config
		let config =
			Config::load_internal(None, None, Some(&dot_config)).expect("Failed to load config");

		// Should skip .config layer and only use project layer -> "standard"
		assert_eq!(config.ssh.host, "standard");
	}

	#[serial]
	#[test]
	fn nested_within_dot_config() {
		let dir = tempdir().unwrap();
		let project = dir.path().join("project");
		let dot_config = project.join(".config");
		let subdir = dot_config.join("subdir");
		fs::create_dir_all(&subdir).unwrap();

		// A config file that would ONLY be found if we treat '.config' as a project layer
		// layer '.config' -> candidates: .config/biwa, .config/.biwa, ...
		// matches .config/.biwa.toml
		//
		// layer 'project' -> candidates: project/biwa, project/.biwa, project/.config/biwa
		// does NOT match project/.config/.biwa.toml (only .config/biwa)
		fs::write(dot_config.join(".biwa.toml"), r#"ssh.host = "weird""#).unwrap();

		let config =
			Config::load_internal(None, None, Some(&subdir)).expect("Failed to load config");

		// Should NOT load "weird" because .config dir should be skipped as a layer
		assert_ne!(config.ssh.host, "weird");
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
	}

	#[serial]
	#[test]
	fn strict_global_config() {
		let dir = tempdir().unwrap();
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa")).unwrap();

		// Multiple global configs should fail
		fs::write(home.join("biwa.toml"), r#"ssh.host = "home""#).unwrap();
		fs::write(config_home.join("biwa/config.toml"), r#"ssh.host = "xdg""#).unwrap();

		let result = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None);
		assert_matches!(result, Err(_));
	}

	#[serial]
	#[test]
	fn strict_local_config() {
		let dir = tempdir().unwrap();
		// Multiple local configs in same dir should fail
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "toml""#).unwrap();
		fs::write(
			dir.path().join(".biwa.json"),
			r#"{"ssh": {"host": "json"}}"#,
		)
		.unwrap();

		let result = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref());
		assert_matches!(result, Err(_));
	}

	#[serial]
	#[test]
	fn conflict_root_and_dot_config() {
		let dir = tempdir().unwrap();
		// Test multiple "local" configs (one within .config) should fail
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "root""#).unwrap();

		let dot_config = dir.path().join(".config");
		fs::create_dir_all(&dot_config).unwrap();
		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "dotconfig""#).unwrap();

		// Should error because we found >1 config for the same dir scope
		let result = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref());
		assert_matches!(result, Err(_));
	}

	#[serial]
	#[test]
	fn local_dot_config_support() {
		let dir = tempdir().unwrap();
		let dot_config = dir.path().join(".config");
		fs::create_dir_all(&dot_config).unwrap();

		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "dotconfig""#).unwrap();

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, "dotconfig");
	}

	#[serial]
	#[test]
	fn ignored_xdg_biwa_biwa() {
		let dir = tempdir().unwrap();
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa")).unwrap();

		// This should be ignored: ~/.config/biwa/biwa.toml
		fs::write(
			config_home.join("biwa/biwa.toml"),
			r#"ssh.host = "ignored""#,
		)
		.unwrap();

		// This is a valid global config: ~/biwa.toml
		// We use this to verify that the other one was indeed ignored and didn't conflict/override.
		fs::write(home.join("biwa.toml"), r#"ssh.host = "fallback""#).unwrap();

		let config = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None)
			.expect("Failed to load config");

		// Should load "fallback", NOT "ignored"
		assert_eq!(config.ssh.host, "fallback");
	}

	#[serial]
	#[test]
	fn find_single_config_logic() {
		let dir = tempdir().unwrap();
		let root = dir.path();

		// 1. No config
		let result = find_single_config(&[root.join("biwa")]);
		assert!(result.unwrap().is_none());

		// 2. Single config
		fs::write(root.join("biwa.toml"), "").unwrap();
		let result = find_single_config(&[root.join("biwa")]);
		let (path, format) = result.unwrap().unwrap();
		assert_eq!(path, root.join("biwa.toml"));
		assert_eq!(format, ConfigFormat::Toml);

		// 3. Multiple formats for same base -> Error (Strictness)
		// Note: find_single_config logic checks across extensions for a single base too?
		// "Multiple configuration files found in the same scope"
		fs::write(root.join("biwa.json"), "{}").unwrap();
		let result = find_single_config(&[root.join("biwa")]);
		assert_matches!(result, Err(_));

		// Cleanup for next check
		fs::remove_file(root.join("biwa.toml")).unwrap();
		fs::remove_file(root.join("biwa.json")).unwrap();

		// 4. Multiple bases in list -> Error if both exist
		// e.g. biwa.toml and .biwa.toml
		fs::write(root.join("biwa.toml"), "").unwrap();
		fs::write(root.join(".biwa.toml"), "").unwrap();
		let result = find_single_config(&[root.join("biwa"), root.join(".biwa")]);
		assert_matches!(result, Err(_));
	}

	#[serial]
	#[test]
	fn nested_path_resolution() {
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

	#[serial]
	#[test]
	fn local_config_root_dot_config_biwa() {
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

	#[serial]
	#[test]
	fn global_config_root_home_and_xdg() {
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

	#[serial]
	#[test]
	fn relative_key_path_resolved_against_source_config() {
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

	#[serial]
	#[test]
	fn load_partial_invalid_toml() -> eyre::Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.toml");
		// Write invalid TOML
		fs::write(&path, "invalid = = toml")?;

		let result = Config::load_partial(&path, ConfigFormat::Toml, dir.path());
		let err = match result {
			Err(e) => e.to_string(),
			Ok(_) => eyre::bail!("Expected parsing error for invalid TOML"),
		};
		eyre::ensure!(
			err.contains("Failed to parse TOML"),
			"Error string mismatch: {}",
			err
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_invalid_yaml() -> eyre::Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.yaml");
		// Write invalid YAML
		fs::write(&path, "invalid:\n  - \n    - :\n")?;

		let result = Config::load_partial(&path, ConfigFormat::Yaml, dir.path());
		let err = match result {
			Err(e) => e.to_string(),
			Ok(_) => eyre::bail!("Expected parsing error for invalid YAML"),
		};
		eyre::ensure!(
			err.contains("Failed to parse YAML"),
			"Error string mismatch: {}",
			err
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_invalid_json() -> eyre::Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.json");
		// Write invalid JSON
		fs::write(&path, r#"{"invalid": true"#)?;

		let result = Config::load_partial(&path, ConfigFormat::Json, dir.path());
		let err = match result {
			Err(e) => e.to_string(),
			Ok(_) => eyre::bail!("Expected parsing error for invalid JSON"),
		};
		eyre::ensure!(
			err.contains("Failed to parse JSON"),
			"Error string mismatch: {}",
			err
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_valid_json5() -> eyre::Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.json5");
		// Write valid JSON5 (with comments and trailing commas)
		fs::write(
			&path,
			r#"
			{
				// This is a comment
				"ssh": {
					"port": 2222,
				}
			}
			"#,
		)?;

		let result = Config::load_partial(&path, ConfigFormat::Json5, dir.path());
		eyre::ensure!(result.is_ok(), "Failed to parse valid JSON5");
		Ok(())
	}
}
