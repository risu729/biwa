#![cfg_attr(
	test,
	allow(clippy::unwrap_used, reason = "too verbose to use expect in tests")
)]
#![cfg_attr(
	test,
	allow(
		clippy::shadow_unrelated,
		reason = "some tests have repeated variable names"
	)
)]
#![cfg_attr(
	test,
	allow(clippy::panic_in_result_fn, reason = "color_eyre handles panics")
)]

extern crate alloc;

#[expect(
	clippy::disallowed_types,
	reason = "This is the crate's central Result type definition."
)]
pub type Result<T> = color_eyre::Result<T>;

/// CLI commands and parsing.
mod cli;
/// Configuration loading and definitions.
mod config;
/// Duration parsing for human-readable time values.
mod duration;
/// Environment variable parsing and forward.
mod env_vars;
/// SSH execution logic.
mod ssh;
/// Local state for connection tracking (`dirs::state_dir` + `/biwa` by default).
mod state;
#[cfg(test)]
mod testing;
/// UI components.
mod ui;

use color_eyre::{Report, config::HookBuilder};
use std::{env, process::ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
	let debug_error_report = env_flag_is_truthy("BIWA_DEBUG_ERROR_REPORT");

	if let Err(error) = install_error_hooks(debug_error_report) {
		eprintln!("{error}");
		return ExitCode::FAILURE;
	}

	match try_main().await {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			print_error(&error, debug_error_report);
			ExitCode::FAILURE
		}
	}
}

/// Runs the CLI and returns any reportable error.
async fn try_main() -> Result<()> {
	cli::run().await
}

/// Installs color-eyre hooks with an optional detailed mode.
fn install_error_hooks(debug_error_report: bool) -> Result<()> {
	let mut hooks = HookBuilder::new().capture_span_trace_by_default(debug_error_report);

	if !debug_error_report {
		hooks = hooks
			.display_env_section(false)
			.display_location_section(false)
			.add_frame_filter(Box::new(|frames| {
				frames.retain(|frame| {
					frame
						.name
						.as_deref()
						.is_none_or(|name| name == "biwa" || name.starts_with("biwa::"))
				});
			}));
	}

	hooks.install()?;
	Ok(())
}

/// Prints an error report based on the selected verbosity mode.
#[expect(
	clippy::use_debug,
	reason = "Detailed mode intentionally renders the full report for troubleshooting."
)]
fn print_error(error: &Report, debug_error_report: bool) {
	if debug_error_report {
		eprintln!("{error:?}");
	} else {
		eprintln!("{error:#}");
	}
}

/// Returns true when an environment variable is set to a truthy value.
fn env_flag_is_truthy(name: &str) -> bool {
	env::var(name)
		.map(|value| {
			matches!(
				value.trim().to_ascii_lowercase().as_str(),
				"1" | "true" | "yes" | "on"
			)
		})
		.unwrap_or(false)
}

#[cfg(test)]
#[ctor::ctor]
fn init_test_env() {
	#[expect(
		clippy::unused_result_ok,
		reason = "Multiple tests may attempt to initialize the global error handler."
	)]
	color_eyre::install().ok();
}
