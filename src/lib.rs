// Main lib.rs file that exports our modules
pub mod config;
pub mod mcp;
pub mod session;
pub mod state;

// Re-export commonly used items for convenience
pub use config::Config;
