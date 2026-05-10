use std::env;

/// Returns true when an environment variable is set to a truthy value.
pub fn is_truthy(name: &str) -> bool {
	env::var(name).is_ok_and(|value| value_is_truthy(&value))
}

/// Returns true for accepted truthy flag values.
fn value_is_truthy(value: &str) -> bool {
	matches!(
		value.trim().to_ascii_lowercase().as_str(),
		"1" | "true" | "yes" | "on"
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::EnvCleanup;
	use serial_test::serial;

	#[test]
	#[serial]
	fn is_truthy_reads_truthy_values() {
		let _cleanup = EnvCleanup::set("BIWA_DEBUG_ERROR_REPORT", " yes ");
		assert!(is_truthy("BIWA_DEBUG_ERROR_REPORT"));
	}

	#[test]
	#[serial]
	fn is_truthy_defaults_to_false_when_missing() {
		let _cleanup = EnvCleanup::remove("BIWA_DEBUG_ERROR_REPORT");
		assert!(!is_truthy("BIWA_DEBUG_ERROR_REPORT"));
	}

	#[test]
	fn recognizes_truthy_values() {
		for value in ["1", "true", "TRUE", " yes ", "on"] {
			assert!(value_is_truthy(value));
		}
	}

	#[test]
	fn rejects_non_truthy_values() {
		for value in ["", "0", "false", "off", "no", "truth"] {
			assert!(!value_is_truthy(value));
		}
	}
}
