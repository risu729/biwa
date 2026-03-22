use alloc::borrow::Cow;
use core::fmt;
use core::str::FromStr;
use core::time::Duration;
use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A human-readable duration wrapper.
///
/// Accepts strings like `30d`, `12h`, `45m`, `60s`, or a plain number which
/// defaults to minutes (e.g. `30` → 30 minutes). Parsing is handled by
/// [`humantime`] with an added convenience for bare numeric values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanDuration(Duration);

impl HumanDuration {
	/// Returns the inner `std::time::Duration`.
	#[must_use]
	pub const fn as_duration(&self) -> Duration {
		self.0
	}
}

impl From<Duration> for HumanDuration {
	fn from(d: Duration) -> Self {
		Self(d)
	}
}

impl Default for HumanDuration {
	fn default() -> Self {
		// Default to 30 days.
		Self(Duration::from_secs(30 * 86400))
	}
}

impl FromStr for HumanDuration {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		// Bare numeric values default to minutes.
		if let Ok(minutes) = s.parse::<u64>() {
			return Ok(Self(Duration::from_secs(minutes.saturating_mul(60))));
		}

		humantime::parse_duration(s)
			.map(Self)
			.map_err(|e| format!("Invalid duration: {s} ({e})"))
	}
}

impl fmt::Display for HumanDuration {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", humantime::format_duration(self.0))
	}
}

impl<'de> Deserialize<'de> for HumanDuration {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let s = String::deserialize(deserializer)?;
		Self::from_str(&s).map_err(D::Error::custom)
	}
}

impl Serialize for HumanDuration {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_str(&self.to_string())
	}
}

impl JsonSchema for HumanDuration {
	fn schema_name() -> Cow<'static, str> {
		Cow::Borrowed("HumanDuration")
	}

	fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
		<String as JsonSchema>::json_schema(generator)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn parse_days() {
		let d: HumanDuration = "30d".parse().unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(30 * 86400));
	}

	#[test]
	fn parse_hours() {
		let d: HumanDuration = "12h".parse().unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(12 * 3600));
	}

	#[test]
	fn parse_minutes() {
		let d: HumanDuration = "45m".parse().unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(45 * 60));
	}

	#[test]
	fn parse_seconds() {
		let d: HumanDuration = "60s".parse().unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(60));
	}

	#[test]
	fn parse_bare_number_defaults_to_minutes() {
		let d: HumanDuration = "30".parse().unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(30 * 60));
	}

	#[test]
	fn parse_invalid_rejected() {
		let result: Result<HumanDuration, _> = "invalid".parse();
		result.unwrap_err();
	}

	#[test]
	fn serde_round_trip() {
		let d: HumanDuration = "5d".parse().unwrap();
		let s = serde_json::to_string(&d).unwrap();
		let d2: HumanDuration = serde_json::from_str(&s).unwrap();
		assert_eq!(d, d2);
	}

	#[test]
	fn deserialize_from_json_string() {
		let d: HumanDuration = serde_json::from_str("\"12h\"").unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(12 * 3600));
	}

	#[test]
	fn deserialize_bare_number_from_json() {
		let d: HumanDuration = serde_json::from_str("\"30\"").unwrap();
		assert_eq!(d.as_duration(), Duration::from_secs(30 * 60));
	}
}
