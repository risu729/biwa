/// SSH authentication processing.
pub mod auth;
/// SSH Client wrapper.
pub mod client;

/// Remote command execution handling.
pub mod exec;
/// SSH file synchronization.
pub mod sync;
/// Helpers shared by SSH synchronization modules.
mod sync_paths;
