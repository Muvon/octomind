pub mod index;
pub mod search;
pub mod view;
pub mod watch;
pub mod config;
pub mod session;
pub mod graphrag;
pub mod clear;

// Re-export all the command structs and enums
pub use index::IndexArgs;
pub use search::SearchArgs;
pub use view::ViewArgs;
pub use watch::WatchArgs;
pub use config::ConfigArgs;
pub use session::SessionArgs;
pub use graphrag::GraphRAGArgs;