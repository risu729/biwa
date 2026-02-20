use super::format::{ConfigFormat, merge_config};
use super::types::Config;
use eyre::Result;
use figment::{Figment, providers::Env};
use std::path::PathBuf;

impl Config {
	pub fn load() -> Result<Self> {
		let home = homedir::my_home()?;
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

			// Load bottom-up (root to deepest) so deeper configs override shallower ones
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
	use std::sync::Mutex;
	use tempfile::tempdir;

	static TEST_MUTEX: Mutex<()> = Mutex::new(());

	#[test]
	fn test_default() {
		let config = Config::default();
		assert_eq!(config.ssh.host, "cse.unsw.edu.au");
		assert_eq!(config.ssh.port, 22);
		assert_eq!(config.ssh.user, "z5555555");
		assert_eq!(
			config.sync.remote_root.relative().to_string_lossy(),
			".cache/biwa/projects"
		);
	}

	#[test]
	fn test_env_override() {
		let _guard = TEST_MUTEX.lock().unwrap();
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("biwa.toml"), r#"ssh.host = "file""#).unwrap();

		// Set env var override
		unsafe {
			std::env::set_var("BIWA_SSH__HOST", "env");
			std::env::set_var("BIWA_SSH__PORT", "8080");
		}

		let config = Config::load_internal(
			Some(&dir.path().to_path_buf()),
			None,
			Some(&dir.path().to_path_buf()),
		)
		.unwrap();

		// Clean up env vars
		unsafe {
			std::env::remove_var("BIWA_SSH__HOST");
			std::env::remove_var("BIWA_SSH__PORT");
		}

		assert_eq!(config.ssh.host, "env"); // Env overrides file
		assert_eq!(config.ssh.port, 8080); // Env override works
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

		let config = Config::load_internal(
			Some(&home.path().to_path_buf()),
			None,
			Some(&inner.clone()),
		)
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

		let config =
			Config::load_internal(Some(&home.clone()), None, Some(&home.clone()))
				.unwrap();
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
		let expected = parent.path().join("my_key").to_string_lossy().into_owned();
		assert_eq!(resolved.relative().to_string_lossy(), expected);
	}
}
