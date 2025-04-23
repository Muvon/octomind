use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Default)]
pub struct IndexState {
	pub current_directory: PathBuf,
	pub indexed_files: usize,
	pub embedding_calls: usize,
	pub indexing_complete: bool,
}

pub type SharedState = Arc<RwLock<IndexState>>;

pub fn create_shared_state() -> SharedState {
	Arc::new(RwLock::new(IndexState::default()))
}
