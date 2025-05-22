use octodev::store::Store;

/// Clear all database tables
pub async fn execute(store: &Store) -> Result<(), anyhow::Error> {
	println!("Clearing all database tables...");
	store.clear_all_tables().await?;
	println!("Successfully cleared all tables.");
	Ok(())
}