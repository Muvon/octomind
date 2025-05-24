// Command executor for /run commands using layers

use crate::config::Config;
use crate::session::{Session, layers::generic_layer::GenericLayer, layers::layer_trait::Layer};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use colored::Colorize;

/// Execute a command layer without storing it in the session history
pub async fn execute_command_layer(
    command_name: &str,
    input: &str,
    session: &Session,
    config: &Config,
    role: &str,
    operation_cancelled: Arc<AtomicBool>
) -> Result<String> {
    // Get role configuration to check for command layers
    let (_, _, _, commands_config, _) = config.get_mode_config(role);
    
    // Find the command configuration
    let command_config = commands_config
        .and_then(|commands| commands.get(command_name))
        .ok_or_else(|| anyhow::anyhow!("Command '{}' not found in configuration", command_name))?;
    
    println!("{} {}", "Executing command:".bright_cyan(), command_name.bright_yellow());
    
    // Create a generic layer with the command configuration
    let command_layer = GenericLayer::new(command_config.clone());
    
    // Execute the layer without affecting the session
    let result = command_layer.process(input, session, config, operation_cancelled).await?;
    
    // Display information about the command execution
    if let Some(usage) = &result.token_usage {
        println!("{} {} prompt, {} completion tokens", 
            "Command usage:".bright_blue(),
            usage.prompt_tokens.to_string().bright_green(),
            usage.completion_tokens.to_string().bright_green());
        
        if let Some(cost) = usage.cost {
            println!("{} ${:.5}", "Command cost:".bright_blue(), cost.to_string().bright_magenta());
        }
    }
    
    Ok(result.output)
}

/// List all available command layers for the current role
pub fn list_available_commands(config: &Config, role: &str) -> Vec<String> {
    let (_, _, _, commands_config, _) = config.get_mode_config(role);
    
    commands_config
        .map(|commands| commands.keys().cloned().collect())
        .unwrap_or_else(Vec::new)
}

/// Check if a command exists for the current role
pub fn command_exists(config: &Config, role: &str, command_name: &str) -> bool {
    let (_, _, _, commands_config, _) = config.get_mode_config(role);
    
    commands_config
        .map(|commands| commands.contains_key(command_name))
        .unwrap_or(false)
}

/// Get help text for command layers
pub fn get_command_help(config: &Config, role: &str) -> String {
    let available_commands = list_available_commands(config, role);
    
    if available_commands.is_empty() {
        "No command layers configured for this role.".to_string()
    } else {
        format!(
            "Available command layers: {}\nUsage: /run <command_name>\nExample: /run estimate",
            available_commands.join(", ")
        )
    }
}