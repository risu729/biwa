use super::format::ConfigFormat;
use super::types::Config;
use crate::Result;
use color_eyre::eyre::{WrapErr as _, bail};
use confique::Config as _;
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

		if let Some(exclude_list) = &mut partial.sync.exclude {
			let root_str = root.display().to_string().replace('\\', "/");
			let root_str = root_str.trim_end_matches('/');
			for glob in exclude_list {
				if !glob.starts_with('/') {
					*glob = format!("{root_str}/{}", glob.trim_start_matches('/'));
				}
			}
		}

		// NOTE: remote_root is intentionally NOT resolved here because it is a remote SSH path.
		// Tilde expansion and relative path resolution should happen on the remote server, not locally.
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
			bail!(
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
	use pretty_assertions::{assert_eq, assert_matches, assert_ne};
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
		assert_eq!(
			config.sync.remote_root,
			PathBuf::from("~/.cache/biwa/projects")
		);
	}

	#[serial]
	#[test]
	fn env_override() -> Result<()> {
		let dir = tempdir()?;
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "file""#)?;

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

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())?;

		assert_eq!(config.ssh.host, "env");
		assert_eq!(config.ssh.port, 8080);
		Ok(())
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
		    "password": false,
		    "umask": "077"
		  },
		  "sync": {
		    "auto": true,
		    "sync_root": null,
		    "remote_root": "~/.cache/biwa/projects",
		    "exclude": [
		      "**/.git/**",
		      "**/target/**",
		      "**/node_modules/**"
		    ],
		    "engine": "sftp",
		    "sftp": {
		      "max_files_to_sync": 100,
		      "permissions": "recreate"
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
	fn format_extensions(
		#[case] content: &str,
		#[case] ext: &str,
		#[case] expected: &str,
	) -> Result<()> {
		let dir = tempdir()?;
		let file_path = dir.path().join(format!("biwa.{ext}"));
		fs::write(&file_path, content)?;

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())?;
		assert_eq!(config.ssh.host, expected);
		Ok(())
	}

	#[serial]
	#[test]
	fn traversal_precedence() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path();
		let subdir = root.join("subdir");
		let nested = subdir.join("nested");
		fs::create_dir_all(&nested)?;

		fs::write(root.join("biwa.toml"), r#"ssh.host = "root""#)?;
		fs::write(subdir.join("biwa.toml"), r#"ssh.host = "subdir""#)?;

		let config = Config::load_internal(None, None, Some(nested).as_ref())?;
		assert_eq!(config.ssh.host, "subdir");
		Ok(())
	}

	#[serial]
	#[test]
	fn traversal_stops_at_home() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path();
		let home = root.join("home");
		let project = home.join("project");
		fs::create_dir_all(&project)?;

		// Config in root (parent of home) - should NOT be loaded if traversal stops at home
		fs::write(root.join("biwa.toml"), r#"ssh.host = "outside""#)?;

		// We need to initialize the home dir so it's a valid path for test logic if needed
		fs::create_dir_all(&home)?;

		let config = Config::load_internal(Some(&home), None, Some(&project))?;

		assert_ne!(config.ssh.host, "outside");
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
		Ok(())
	}

	#[serial]
	#[test]
	fn xdg_precedence() -> Result<()> {
		let dir = tempdir()?;
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa"))?;

		fs::write(config_home.join("biwa/config.toml"), r#"ssh.host = "xdg""#)?;

		let config = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None)?;
		assert_eq!(config.ssh.host, "xdg");
		Ok(())
	}

	#[serial]
	#[test]
	fn cwd_is_dot_config() -> Result<()> {
		let dir = tempdir()?;
		let project = dir.path().join("project");
		let dot_config = project.join(".config");
		let biwa_dir = dot_config.join("biwa");
		fs::create_dir_all(&biwa_dir)?;

		// Standard config locatable from 'project' layer
		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "standard""#)?;

		// Weird config only locatable if '.config' is a layer
		fs::write(dot_config.join(".biwa.toml"), r#"ssh.host = "weird""#)?;

		// CWD is .config
		let config = Config::load_internal(None, None, Some(&dot_config))?;

		// Should skip .config layer and only use project layer -> "standard"
		assert_eq!(config.ssh.host, "standard");
		Ok(())
	}

	#[serial]
	#[test]
	fn nested_within_dot_config() -> Result<()> {
		let dir = tempdir()?;
		let project = dir.path().join("project");
		let dot_config = project.join(".config");
		let subdir = dot_config.join("subdir");
		fs::create_dir_all(&subdir)?;

		// A config file that would ONLY be found if we treat '.config' as a project layer
		// layer '.config' -> candidates: .config/biwa, .config/.biwa, ...
		// matches .config/.biwa.toml
		//
		// layer 'project' -> candidates: project/biwa, project/.biwa, project/.config/biwa
		// does NOT match project/.config/.biwa.toml (only .config/biwa)
		fs::write(dot_config.join(".biwa.toml"), r#"ssh.host = "weird""#)?;

		let config = Config::load_internal(None, None, Some(&subdir))?;

		// Should NOT load "weird" because .config dir should be skipped as a layer
		assert_ne!(config.ssh.host, "weird");
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
		Ok(())
	}

	#[serial]
	#[test]
	fn strict_global_config() -> Result<()> {
		let dir = tempdir()?;
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa"))?;

		// Multiple global configs should fail
		fs::write(home.join("biwa.toml"), r#"ssh.host = "home""#)?;
		fs::write(config_home.join("biwa/config.toml"), r#"ssh.host = "xdg""#)?;

		let result = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None);
		assert_matches!(result, Err(_));
		Ok(())
	}

	#[serial]
	#[test]
	fn strict_local_config() -> Result<()> {
		let dir = tempdir()?;
		// Multiple local configs in same dir should fail
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "toml""#)?;
		fs::write(
			dir.path().join(".biwa.json"),
			r#"{"ssh": {"host": "json"}}"#,
		)?;

		let result = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref());
		assert_matches!(result, Err(_));
		Ok(())
	}

	#[serial]
	#[test]
	fn conflict_root_and_dot_config() -> Result<()> {
		let dir = tempdir()?;
		// Test multiple "local" configs (one within .config) should fail
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "root""#)?;

		let dot_config = dir.path().join(".config");
		fs::create_dir_all(&dot_config)?;
		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "dotconfig""#)?;

		// Should error because we found >1 config for the same dir scope
		let result = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref());
		assert_matches!(result, Err(_));
		Ok(())
	}

	#[serial]
	#[test]
	fn local_dot_config_support() -> Result<()> {
		let dir = tempdir()?;
		let dot_config = dir.path().join(".config");
		fs::create_dir_all(&dot_config)?;

		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "dotconfig""#)?;

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())?;
		assert_eq!(config.ssh.host, "dotconfig");
		Ok(())
	}

	#[serial]
	#[test]
	fn ignored_xdg_biwa_biwa() -> Result<()> {
		let dir = tempdir()?;
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa"))?;

		// This should be ignored: ~/.config/biwa/biwa.toml
		fs::write(
			config_home.join("biwa/biwa.toml"),
			r#"ssh.host = "ignored""#,
		)?;

		// This is a valid global config: ~/biwa.toml
		// We use this to verify that the other one was indeed ignored and didn't conflict/override.
		fs::write(home.join("biwa.toml"), r#"ssh.host = "fallback""#)?;

		let config = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None)?;

		// Should load "fallback", NOT "ignored"
		assert_eq!(config.ssh.host, "fallback");
		Ok(())
	}

	#[serial]
	#[test]
	fn find_single_config_logic() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path();

		// 1. No config
		let result = find_single_config(&[root.join("biwa")]);
		assert!(result?.is_none());

		// 2. Single config
		fs::write(root.join("biwa.toml"), "")?;
		let result = find_single_config(&[root.join("biwa")]);
		let (path, format) = result?.ok_or_else(|| color_eyre::eyre::eyre!("Expected Some"))?;
		assert_eq!(path, root.join("biwa.toml"));
		assert_eq!(format, ConfigFormat::Toml);

		// 3. Multiple formats for same base -> Error (Strictness)
		// Note: find_single_config logic checks across extensions for a single base too?
		// "Multiple configuration files found in the same scope"
		fs::write(root.join("biwa.json"), "{}")?;
		let result = find_single_config(&[root.join("biwa")]);
		assert_matches!(result, Err(_));

		// Cleanup for next check
		fs::remove_file(root.join("biwa.toml"))?;
		fs::remove_file(root.join("biwa.json"))?;

		// 4. Multiple bases in list -> Error if both exist
		// e.g. biwa.toml and .biwa.toml
		fs::write(root.join("biwa.toml"), "")?;
		fs::write(root.join(".biwa.toml"), "")?;
		let result = find_single_config(&[root.join("biwa"), root.join(".biwa")]);
		assert_matches!(result, Err(_));
		Ok(())
	}

	#[serial]
	#[test]
	fn nested_path_resolution() -> Result<()> {
		let dir = tempdir()?;
		let root = dir.path();
		let subdir = root.join("subdir");
		fs::create_dir_all(&subdir)?;

		// Parent config defines a remote path
		// remote_root is a remote SSH path, so it should NOT be resolved locally
		fs::write(
			root.join("biwa.toml"),
			r#"
[sync]
remote_root = "libs"
"#,
		)?;

		// Child config overrides something else, but inherits remote_root
		fs::write(
			subdir.join("biwa.toml"),
			r#"
[ssh]
host = "child"
"#,
		)?;

		// Load config from subdir
		let config = Config::load_internal(None, None, Some(&subdir))?;

		// remote_root should remain as the raw value from the config file
		assert_eq!(
			config.sync.remote_root,
			PathBuf::from("libs"),
			"remote_root is a remote SSH path and should not be resolved locally"
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn local_config_root_dot_config_biwa() -> Result<()> {
		let dir = tempdir()?;
		let project = dir.path().join("project");
		let dot_config = project.join(".config");
		fs::create_dir_all(&dot_config)?;

		// remote_root is a remote SSH path, so it should NOT be resolved locally
		fs::write(
			dot_config.join("biwa.toml"),
			r#"
[sync]
remote_root = "libs"
"#,
		)?;

		let config = Config::load_internal(None, None, Some(&project))?;

		assert_eq!(
			config.sync.remote_root,
			PathBuf::from("libs"),
			"remote_root is a remote SSH path and should not be resolved locally"
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn global_config_root_home_and_xdg() -> Result<()> {
		let dir = tempdir()?;
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(&home)?;
		fs::create_dir_all(config_home.join("biwa"))?;

		// Global config at ~/biwa.toml — remote_root is a remote path, should not be resolved locally
		fs::write(
			home.join("biwa.toml"),
			r#"
[sync]
remote_root = "global_libs"
"#,
		)?;

		let config = Config::load_internal(Some(&home), Some(&config_home), None)?;
		assert_eq!(
			config.sync.remote_root,
			PathBuf::from("global_libs"),
			"remote_root is a remote SSH path and should not be resolved locally"
		);

		// Only one global config is allowed; remove the home config before testing the XDG variant.
		fs::remove_file(home.join("biwa.toml"))?;

		// Override with XDG-style global config at ~/.config/biwa/config.toml
		fs::write(
			config_home.join("biwa/config.toml"),
			r#"
[sync]
remote_root = "xdg_libs"
"#,
		)?;

		let config = Config::load_internal(Some(&home), Some(&config_home), None)?;
		assert_eq!(
			config.sync.remote_root,
			PathBuf::from("xdg_libs"),
			"remote_root is a remote SSH path and should not be resolved locally"
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn relative_key_path_resolved_against_source_config() -> Result<()> {
		// Layout:
		//   /parent/biwa.toml       -> sets ssh.key_path = "my_key"
		//   /parent/my_key          -> the key file
		//   /parent/child/biwa.toml -> overrides ssh.host only
		let parent = tempdir()?;
		let child = parent.path().join("child");
		fs::create_dir_all(&child)?;

		fs::write(
			parent.path().join("biwa.toml"),
			"[ssh]\nkey_path = \"my_key\"\n",
		)?;
		fs::write(parent.path().join("my_key"), "fake key")?;
		fs::write(child.join("biwa.toml"), "[ssh]\nhost = \"other.host\"\n")?;

		let config = Config::load_internal(None, None, Some(&child))?;

		// key_path should be resolved to parent/my_key, not child/my_key
		let resolved = config
			.ssh
			.key_path
			.ok_or_else(|| color_eyre::eyre::eyre!("key_path should be set"))?;
		let expected = parent.path().join("my_key");
		assert_eq!(resolved, expected);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_invalid_toml() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.toml");
		// Write invalid TOML
		fs::write(&path, "invalid = = toml")?;

		let result = Config::load_partial(&path, ConfigFormat::Toml, dir.path());
		let err = match result {
			Err(e) => e.to_string(),
			Ok(_) => bail!("Expected parsing error for invalid TOML"),
		};
		assert!(
			err.contains("Failed to parse TOML"),
			"Error string mismatch: {err}"
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_invalid_yaml() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.yaml");
		// Write invalid YAML
		fs::write(&path, "invalid:\n  - \n    - :\n")?;

		let result = Config::load_partial(&path, ConfigFormat::Yaml, dir.path());
		let err = match result {
			Err(e) => e.to_string(),
			Ok(_) => bail!("Expected parsing error for invalid YAML"),
		};
		assert!(
			err.contains("Failed to parse YAML"),
			"Error string mismatch: {err}"
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_invalid_json() -> Result<()> {
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("config.json");
		// Write invalid JSON
		fs::write(&path, r#"{"invalid": true"#)?;

		let result = Config::load_partial(&path, ConfigFormat::Json, dir.path());
		let err = match result {
			Err(e) => e.to_string(),
			Ok(_) => bail!("Expected parsing error for invalid JSON"),
		};
		assert!(
			err.contains("Failed to parse JSON"),
			"Error string mismatch: {err}"
		);
		Ok(())
	}

	#[serial]
	#[test]
	fn load_partial_valid_json5() -> Result<()> {
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
		assert!(result.is_ok(), "Failed to parse valid JSON5");
		Ok(())
	}
}
