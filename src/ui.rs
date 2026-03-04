use core::time::Duration;
use indicatif::{ProgressBar, ProgressStyle};

/// Creates a new spinner with a consistent style across the application.
///
/// # Panics
/// Panics if the default spinner template is invalid.
pub fn create_spinner(message: impl Into<String>) -> ProgressBar {
	let sp = ProgressBar::new_spinner();
	sp.set_style(
		ProgressStyle::default_spinner()
			.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
			.template("{spinner:.cyan} {msg}")
			.expect("invalid spinner template"),
	);
	sp.set_message(message.into());
	sp.enable_steady_tick(Duration::from_millis(80));
	sp
}
