/// SSH authentication processing.
pub mod auth;
/// Remote cleanup operations.
pub mod clean;
/// SSH Client wrapper.
pub mod client;

/// Remote command execution handling.
pub mod exec;
/// SSH file synchronization.
pub mod sync;
/// Local sync state caching for incremental synchronization.
mod sync_cache;
/// Helpers shared by SSH synchronization modules.
mod sync_paths;
/// Effective SSH target resolution from Biwa and OpenSSH configuration.
pub mod target;
