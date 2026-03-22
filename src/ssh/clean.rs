use crate::Result;
use crate::ssh::client::Client;
use crate::ssh::sync::shell_quote_path;
use color_eyre::eyre::{Context as _, bail};
use tracing::{debug, info, warn};

/// Parsed quota usage from the remote `quota -w` command.
#[derive(Debug, Clone)]
pub struct QuotaUsage {
	/// Used disk blocks (in kilobytes).
	pub blocks_used: u64,
	/// Soft quota limit in blocks.
	pub blocks_quota: u64,
}

impl QuotaUsage {
	/// Returns the disk usage percentage relative to the soft quota.
	#[must_use]
	#[expect(
		clippy::cast_precision_loss,
		clippy::as_conversions,
		reason = "Block counts are only used to form a ratio; f64 precision is sufficient for quota percentages"
	)]
	pub fn usage_percent(&self) -> f64 {
		if self.blocks_quota == 0 {
			return 0.0;
		}
		let used = self.blocks_used as f64;
		let quota = self.blocks_quota as f64;
		(used / quota) * 100.0
	}
}

/// Parses `quota -w` output to extract usage information.
///
/// Expected format (Linux `quota` command with `-w` flag):
/// ```text
/// Disk quotas for user z1234567 (uid 12345):
///      Filesystem  blocks   quota   limit   grace   files   quota   limit   grace
/// reed:/export/reed/8 3156648  3190784 3509864          109859  319080  350988
/// ```
fn parse_quota_output(output: &str) -> Option<QuotaUsage> {
	for line in output.lines() {
		let line = line.trim();
		// Skip header and empty lines.
		if line.is_empty()
			|| line.starts_with("Disk quotas")
			|| line.starts_with("Filesystem")
			|| line.contains("blocks")
		{
			continue;
		}

		// Data lines start with a filesystem path. Fields are space-separated.
		// Format: filesystem blocks quota limit [grace] files quota limit [grace]
		let fields: Vec<&str> = line.split_whitespace().collect();
		// We need at least filesystem + blocks + quota (3 fields).
		if fields.len() < 3 {
			continue;
		}

		// First field is the filesystem, followed by numeric values.
		// Find the first numeric value which is blocks_used.
		let numeric_start = fields.iter().position(|f| f.parse::<u64>().is_ok())?;
		let blocks_used: u64 = fields.get(numeric_start)?.parse().ok()?;
		let blocks_quota: u64 = fields.get(numeric_start.checked_add(1)?)?.parse().ok()?;

		return Some(QuotaUsage {
			blocks_used,
			blocks_quota,
		});
	}

	None
}

/// Runs `quota -w` on the remote host and parses the result.
pub async fn check_quota(client: &Client) -> Result<Option<QuotaUsage>> {
	let result = client
		.execute("quota -w 2>/dev/null")
		.await
		.wrap_err("Failed to run quota command")?;

	if result.exit_status != 0 {
		debug!(
			exit_status = result.exit_status,
			"quota command failed; quota-based cleanup will be skipped"
		);
		return Ok(None);
	}

	Ok(parse_quota_output(&result.stdout))
}

/// Lists directories directly under the given remote root.
pub async fn list_remote_dirs(client: &Client, remote_root: &str) -> Result<Vec<String>> {
	let quoted_root = shell_quote_path(remote_root);
	let script = format!(
		"if [ -d {quoted_root} ]; then find -- {quoted_root} -mindepth 1 -maxdepth 1 -type d -printf '%f\\n' 2>/dev/null; fi"
	);

	let result = client
		.execute(&script)
		.await
		.wrap_err("Failed to list remote directories")?;

	if result.exit_status != 0 {
		warn!(
			exit_status = result.exit_status,
			stderr = result.stderr.trim(),
			"Failed to list remote directories"
		);
		return Ok(Vec::new());
	}

	let dirs = result
		.stdout
		.lines()
		.filter(|line| !line.trim().is_empty())
		.map(String::from)
		.collect();

	Ok(dirs)
}

/// Removes a remote directory via SSH `rm -rf`.
pub async fn remove_remote_dir(client: &Client, remote_dir: &str) -> Result<()> {
	let quoted = shell_quote_path(remote_dir);
	let cmd = format!("rm -rf -- {quoted}");
	info!(remote_dir, "Removing remote directory");

	let result = client
		.execute(&cmd)
		.await
		.wrap_err_with(|| format!("Failed to remove remote directory: {remote_dir}"))?;

	if result.exit_status != 0 {
		bail!(
			"Failed to remove remote directory {}: {}",
			remote_dir,
			result.stderr.trim()
		);
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn parse_quota_standard_format() {
		let output = "\
Disk quotas for user z5642102 (uid 26573): 
     Filesystem  blocks   quota   limit   grace   files   quota   limit   grace
reed:/export/reed/8 3156648  3190784 3509864          109859  319080  350988        
";
		let usage = parse_quota_output(output).unwrap();
		assert_eq!(usage.blocks_used, 3_156_648);
		assert_eq!(usage.blocks_quota, 3_190_784);
		let pct = usage.usage_percent();
		assert!(pct > 98.0 && pct < 100.0, "got {pct}");
	}

	#[test]
	fn parse_quota_empty_output() {
		assert!(parse_quota_output("").is_none());
	}

	#[test]
	fn parse_quota_headers_only() {
		let output = "\
Disk quotas for user test (uid 1000): 
     Filesystem  blocks   quota   limit   grace   files   quota   limit   grace
";
		assert!(parse_quota_output(output).is_none());
	}

	#[test]
	fn usage_percent_zero_quota() {
		let usage = QuotaUsage {
			blocks_used: 100,
			blocks_quota: 0,
		};
		let pct = usage.usage_percent();
		assert!((pct - 0.0).abs() < f64::EPSILON);
	}

	#[test]
	fn usage_percent_half() {
		let usage = QuotaUsage {
			blocks_used: 500,
			blocks_quota: 1000,
		};
		let pct = usage.usage_percent();
		assert!((pct - 50.0).abs() < f64::EPSILON);
	}

	#[test]
	fn usage_percent_large_block_counts() {
		let usage = QuotaUsage {
			blocks_used: u64::from(u32::MAX) + 10,
			blocks_quota: (u64::from(u32::MAX) + 10) * 2,
		};
		let pct = usage.usage_percent();
		assert!((pct - 50.0).abs() < 1e-6, "got {pct}");
	}
}
