use super::format::{ConfigFormat, merge_config};
use super::types::Config;
use eyre::Result;
use figment::{Figment, providers::Env};
use std::path::PathBuf;

impl Config {
	pub fn load() -> Result<Self> {
		let home = std::env::var("HOME").ok().map(PathBuf::from);
		let xdg = std::env::var("XDG_CONFIG_HOME").ok().map(PathBuf::from);
		let cwd = std::env::current_dir().ok();
		Self::load_internal(home.as_ref(), xdg.as_ref(), cwd.as_ref())
	}

	fn load_internal(
		home: Option<&PathBuf>,
		xdg: Option<&PathBuf>,
		cwd: Option<&PathBuf>,
	) -> Result<Self> {
		let mut figment = Figment::new();

		let mut global_candidates = Vec::new();
		if let Some(home_path) = home {
			global_candidates.push(home_path.join("biwa"));
			global_candidates.push(home_path.join(".biwa"));
			let config_home = xdg.cloned().unwrap_or_else(|| home_path.join(".config"));
			global_candidates.push(config_home.join("biwa/config"));
		}

		if let Some((config_path, format)) = find_single_config(&global_candidates)? {
			figment = merge_config(figment, &config_path, format);
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

			for path in layers.iter().rev() {
				// Don't load config from .config directory itself
				if path.file_name().and_then(|s| s.to_str()) == Some(".config") {
					continue;
				}

				let local_candidates = vec![
					path.join("biwa"),
					path.join(".biwa"),
					path.join(".config/biwa"),
				];
				if let Some((config_path, format)) = find_single_config(&local_candidates)? {
					figment = merge_config(figment, &config_path, format);
				}
			}
		}

		figment = figment.merge(Env::prefixed("BIWA_").split("__"));

		let config: Config = figment.extract()?;
		Ok(config)
	}
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
	use tempfile::tempdir;

	#[test]
	fn test_default() {
		let config = Config::default();
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
		assert_eq!(config.ssh.port, 22);
		assert_eq!(config.ssh.user, "z1234567");
		assert_eq!(config.sync.remote_root, "~/.cache/biwa/projects");
	}

	#[test]
	fn test_env_override() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "file""#).unwrap();

		// Set env var override
		unsafe {
			std::env::set_var("BIWA_SSH__HOST", "env");
		}

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())
			.expect("Failed to load config");

		// Clean up env var before assertion so it doesn't leak if assertion fails?
		// Better to clean up in a finally block or ensuring logic.
		// For simple test, we just remove it after load.
		unsafe {
			std::env::remove_var("BIWA_SSH__HOST");
		}

		assert_eq!(config.ssh.host, "env");
	}

	#[test]
	fn test_snapshot() {
		let config = Config::default();
		insta::assert_json_snapshot!(config, @r###"
  {
    "ssh": {
      "host": "cse.unsw.edu.au",
      "port": 22,
      "user": "z1234567",
      "key_path": null
    },
    "sync": {
      "remote_root": "~/.cache/biwa/projects",
      "ignore_files": [
        ".git",
        "target",
        "node_modules"
      ]
    },
    "env": {
      "vars": []
    },
    "hooks": {
      "pre_sync": null,
      "post_sync": null
    }
  }
  "###);
	}

	#[rstest]
	#[case::toml("ssh.host = 'toml'", "toml", "toml")]
	#[case::json(r#"{ "ssh": { "host": "json" } }"#, "json", "json")]
	#[case::json5("{ ssh: { host: 'json5' } }", "json5", "json5")]
	#[case::yaml("ssh:\n  host: yaml", "yaml", "yaml")]
	fn test_format_extensions(#[case] content: &str, #[case] ext: &str, #[case] expected: &str) {
		let dir = tempdir().unwrap();
		let file_path = dir.path().join(format!("biwa.{ext}"));
		fs::write(&file_path, content).unwrap();

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, expected);
	}

	#[test]
	fn test_traversal_precedence() {
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

	#[test]
	fn test_traversal_stops_at_home() {
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

	#[test]
	fn test_xdg_precedence() {
		let dir = tempdir().unwrap();
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa")).unwrap();

		fs::write(config_home.join("biwa/config.toml"), r#"ssh.host = "xdg""#).unwrap();

		let config = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None)
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, "xdg");
	}

	#[test]
	fn test_cwd_is_dot_config() {
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

	#[test]
	fn test_nested_within_dot_config() {
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

	#[test]
	fn test_strict_global_config() {
		let dir = tempdir().unwrap();
		let home = dir.path().join("home");
		let config_home = home.join(".config");
		fs::create_dir_all(config_home.join("biwa")).unwrap();

		// Multiple global configs should fail
		fs::write(home.join("biwa.toml"), r#"ssh.host = "home""#).unwrap();
		fs::write(config_home.join("biwa/config.toml"), r#"ssh.host = "xdg""#).unwrap();

		let result = Config::load_internal(Some(home).as_ref(), Some(config_home).as_ref(), None);
		assert!(result.is_err());
	}

	#[test]
	fn test_strict_local_config() {
		let dir = tempdir().unwrap();
		// Multiple local configs in same dir should fail
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "toml""#).unwrap();
		fs::write(
			dir.path().join(".biwa.json"),
			r#"{"ssh": {"host": "json"}}"#,
		)
		.unwrap();

		let result = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref());
		assert!(result.is_err());
	}

	#[test]
	fn test_conflict_root_and_dot_config() {
		let dir = tempdir().unwrap();
		// Test multiple "local" configs (one within .config) should fail
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "root""#).unwrap();

		let dot_config = dir.path().join(".config");
		fs::create_dir_all(&dot_config).unwrap();
		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "dotconfig""#).unwrap();

		// Should error because we found >1 config for the same dir scope
		let result = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref());
		assert!(result.is_err());
	}

	#[test]
	fn test_local_dot_config_support() {
		let dir = tempdir().unwrap();
		let dot_config = dir.path().join(".config");
		fs::create_dir_all(&dot_config).unwrap();

		fs::write(dot_config.join("biwa.toml"), r#"ssh.host = "dotconfig""#).unwrap();

		let config = Config::load_internal(None, None, Some(dir.path().to_path_buf()).as_ref())
			.expect("Failed to load config");
		assert_eq!(config.ssh.host, "dotconfig");
	}

	#[test]
	fn test_ignored_xdg_biwa_biwa() {
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

	#[test]
	fn test_find_single_config_logic() {
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
}
