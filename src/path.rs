use std::path::PathBuf;
use tracing::debug;

/// Expand `~` prefix to the user's home directory.
///
/// Uses [`homedir::my_home`] for cross-platform home directory resolution.
/// Returns the path unchanged if it doesn't start with `~/` or if the home
/// directory cannot be determined.
pub fn expand_tilde(path: &str) -> PathBuf {
	if let Some(rest) = path.strip_prefix("~/") {
		match homedir::my_home() {
			Ok(Some(home)) => return home.join(rest),
			Ok(None) => debug!("Home directory not found"),
			Err(e) => debug!(error = %e, "Failed to determine home directory"),
		}
	}
	PathBuf::from(path)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_expand_tilde() {
		let result = expand_tilde("~/.ssh/id_rsa");
		if let Ok(Some(home)) = homedir::my_home() {
			assert_eq!(result, home.join(".ssh/id_rsa"));
		}
	}

	#[test]
	fn test_expand_tilde_no_prefix() {
		let result = expand_tilde("/absolute/path");
		assert_eq!(result, PathBuf::from("/absolute/path"));
	}

	#[test]
	fn test_expand_tilde_just_tilde() {
		// "~" without "/" should not expand
		let result = expand_tilde("~");
		assert_eq!(result, PathBuf::from("~"));
	}

	#[test]
	fn test_expand_tilde_relative() {
		let result = expand_tilde("relative/path");
		assert_eq!(result, PathBuf::from("relative/path"));
	}
}
