// Indexer integration for sessions

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::process::{Child, Command, Stdio};
use tokio::sync::Mutex;
use anyhow::Result;
use crate::state;
use crate::store::Store;
use crate::config::Config;
use crate::indexer;
use std::env;
use std::fs;
use std::path::Path;

// A handle to a running watcher process that can be stopped when the session ends
pub struct WatcherHandle {
    running: Arc<AtomicBool>,
    process: Option<Child>,
}

// Initialize as a static to ensure it's cleaned up on program exit
lazy_static::lazy_static! {
    static ref WATCHER_HANDLE: Arc<Mutex<Option<WatcherHandle>>> = Arc::new(Mutex::new(None));
}

impl WatcherHandle {
    pub fn new(process: Option<Child>, running: Arc<AtomicBool>) -> Self {
        Self { process, running }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    // Stops the watcher if it's running
    pub fn stop(&mut self) -> Result<()> {
        // First set running to false to signal any background tasks to stop
        self.running.store(false, Ordering::SeqCst);

        // Then kill the process if it exists
        if let Some(ref mut process) = self.process {
            // Kill the process
            match process.kill() {
                Ok(_) => {
                    // Don't print anything to keep the session clean
                },
                Err(e) => {
                    // Only print errors
                    eprintln!("Failed to kill watcher process: {}", e);
                },
            }
        }

        Ok(())
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.stop();
        }
    }
}

// Ensure the watcher is stopped when the program exits
pub async fn cleanup_watcher() -> Result<()> {
    let mut guard = WATCHER_HANDLE.lock().await;
    if let Some(ref mut handle) = *guard {
        handle.stop()?;
    }
    *guard = None;
    Ok(())
}

// Index the current directory and return once indexing is complete
pub async fn index_current_directory(store: &Store, config: &Config) -> Result<()> {
    println!("Indexing current directory before starting session...");

    let current_dir = std::env::current_dir()?;
    let state = state::create_shared_state();
    state.write().current_directory = current_dir.clone();

    // Check if index already exists
    let octodev_dir = current_dir.join(".octodev");
    let index_path = octodev_dir.join("storage");
    let index_exists = index_path.exists() && index_path.is_dir();

    // Check if we need to check for updated files
    if index_exists {
        // Get the timestamp of the most recently modified file
        let most_recent = find_most_recent_file_time(&current_dir)?;

        // Get the timestamp of the index (using the directory's mtime)
        let index_modified = fs::metadata(&index_path)?.modified()?.elapsed()?.as_secs();

        // If the index is fresher than the most recent file, we don't need to index
        if most_recent > index_modified {
            println!("Index is up-to-date, skipping indexing...");
            return Ok(());
        }
    }

    // Start indexing
    indexer::index_files(store, state.clone(), config).await?;

    println!("✓ Indexing complete!");
    Ok(())
}

// Find the most recent modification time of any file in the directory
fn find_most_recent_file_time(dir: &Path) -> Result<u64> {
    let mut most_recent = 0;
    let ignored = ["target", ".git", ".octodev"];

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return Err(anyhow::anyhow!("Failed to read directory: {}", e)),
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();

        // Skip ignored directories
        if path.is_dir() && ignored.contains(&file_name.as_ref()) {
            continue;
        }

        // If it's a file, check its modified time
        if path.is_file() {
            if let Ok(metadata) = fs::metadata(&path) {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(elapsed) = modified.elapsed() {
                        let seconds = elapsed.as_secs();
                        if seconds > most_recent {
                            most_recent = seconds;
                        }
                    }
                }
            }
        }

        // If it's a directory, recurse
        if path.is_dir() {
            if let Ok(dir_recent) = find_most_recent_file_time(&path) {
                if dir_recent > most_recent {
                    most_recent = dir_recent;
                }
            }
        }
    }

    Ok(most_recent)
}

// Start the watcher as a background task within the same process
pub async fn start_watcher_in_background(_store: &Store, _config: &Config) -> Result<()> {
    // First clean up any existing watcher
    cleanup_watcher().await?;

    // Use the command-line approach which is safer for this use case
    let handle = start_watcher_process()?;

    // Store the watcher handle
    *WATCHER_HANDLE.lock().await = Some(handle);

    println!("Background watcher started");
    Ok(())
}

// Start the watcher in a separate process with output redirected to null
pub fn start_watcher_process() -> Result<WatcherHandle> {
    let current_exe = env::current_exe()?;

    // Create a log file for watcher output
    let octodev_dir = env::current_dir()?.join(".octodev");
    if !octodev_dir.exists() {
        fs::create_dir_all(&octodev_dir)?;
    }

    let log_path = octodev_dir.join("watcher.log");
    let log_file = fs::File::create(log_path)?;

    // Construct the command to run the watcher in a new process
    let mut command = Command::new(current_exe);
    command
        .arg("watch")
        .arg("--quiet") // Add this flag to reduce output, assuming you'll add support for it
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file));

    // Start the process
    let process = command.spawn()?;

    let running = Arc::new(AtomicBool::new(true));
    let handle = WatcherHandle::new(Some(process), running);

    // Don't print to keep the session clean
    Ok(handle)
}

// Stop the currently running watcher
pub async fn stop_watcher() -> Result<()> {
    let mut guard = WATCHER_HANDLE.lock().await;
    if let Some(ref mut handle) = *guard {
        handle.stop()?;
        // Don't print to keep the session clean
    }
    *guard = None;
    Ok(())
}
