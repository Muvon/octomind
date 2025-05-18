// MCP local server process manager

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use anyhow::Result;
use tokio::time::sleep;
use crate::config::McpServerConfig;

// Global process registry to keep track of running server processes
lazy_static::lazy_static! {
    static ref SERVER_PROCESSES: Arc<RwLock<HashMap<String, Arc<Mutex<Child>>>>> = 
        Arc::new(RwLock::new(HashMap::new()));
}

// Start a local MCP server process if not already running
pub async fn ensure_server_running(server: &McpServerConfig) -> Result<String> {
    let server_id = server.name.clone();
    
    // Check if the server is already running
    {
        let processes = SERVER_PROCESSES.read().unwrap();
        if processes.contains_key(&server_id) {
            // Server is already running
            return get_server_url(server);
        }
    }
    
    // If we get here, we need to start the server
    start_server_process(server).await
}

// Start a server process based on configuration
async fn start_server_process(server: &McpServerConfig) -> Result<String> {
    // Get command and args from config
    let command = server.command.as_ref()
        .ok_or_else(|| anyhow::anyhow!("Command not specified for local MCP server: {}", server.name))?;
    
    // Build and start the command
    let mut cmd = Command::new(command);
    
    // Add arguments if present
    if !server.args.is_empty() {
        cmd.args(&server.args);
    }
    
    // Configure standard I/O
    cmd.stdout(Stdio::piped())
       .stderr(Stdio::piped());
    
    // Start the process
    println!("Starting MCP server: {}", server.name);
    let child = cmd.spawn()
        .map_err(|e| anyhow::anyhow!("Failed to start MCP server '{}': {}", server.name, e))?;
    
    // Add to the registry
    {
        let mut processes = SERVER_PROCESSES.write().unwrap();
        processes.insert(server.name.clone(), Arc::new(Mutex::new(child)));
    }
    
    // Wait a moment to let the server start
    let start_time = Instant::now();
    let max_wait = Duration::from_secs(10); // Maximum 10 seconds to wait for server to start
    
    // For local servers, we assume they're running on localhost on some port
    // The URL could be specified in the configuration or we use a default
    let server_url = get_server_url(server)?;
    
    // Wait for the server to be available
    loop {
        // If it's been too long, give up
        if start_time.elapsed() > max_wait {
            return Err(anyhow::anyhow!("Timed out waiting for MCP server to start: {}", server.name));
        }
        
        // Try to connect to the server
        if can_connect(&server_url).await {
            println!("MCP server started: {} at {}", server.name, server_url);
            return Ok(server_url);
        }
        
        // Wait a bit before trying again
        sleep(Duration::from_millis(500)).await;
    }
}

// Try to connect to a server to see if it's running
async fn can_connect(url: &str) -> bool {
    // Simple HTTP request to check if server is responding
    match reqwest::Client::new().get(url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false
    }
}

// Get the URL for a server based on configuration
fn get_server_url(server: &McpServerConfig) -> Result<String> {
    // If URL is explicitly specified, use that
    if let Some(url) = &server.url {
        return Ok(url.clone());
    }
    
    // Otherwise, assume it's running on localhost
    // For now we use a default port, but ideally this would be configurable
    // or the server would output its port when starting
    Ok(format!("http://localhost:8008"))
}

// Stop all running server processes
pub fn stop_all_servers() -> Result<()> {
    let mut processes = SERVER_PROCESSES.write().unwrap();
    
    for (name, child_arc) in processes.iter() {
        let mut child = child_arc.lock().unwrap();
        println!("Stopping MCP server: {}", name);
        if let Err(e) = child.kill() {
            eprintln!("Failed to kill MCP server '{}': {}", name, e);
        }
    }
    
    processes.clear();
    Ok(())
}

// Check if a server process is still running
pub fn is_server_running(server_name: &str) -> bool {
    let processes = SERVER_PROCESSES.read().unwrap();
    if let Some(child_arc) = processes.get(server_name) {
        let mut child = child_arc.lock().unwrap();
        child.try_wait().map(|status| status.is_none()).unwrap_or(false)
    } else {
        false
    }
}