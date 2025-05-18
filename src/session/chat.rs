// Chat session implementation

use crate::config::Config;
use crate::store::Store;
use super::{Session, get_sessions_dir, load_session, create_system_prompt, openrouter};
use crossterm::{cursor, execute};
use std::io::{self, Write, stdout};
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use ctrlc;
use rustyline::error::ReadlineError;
use rustyline::{Editor, Config as RustylineConfig, CompletionType, EditMode};

// Model choices
pub const CLAUDE_MODEL: &str = "anthropic/claude-3-sonnet-20240229";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;

// Chat commands
const HELP_COMMAND: &str = "/help";
const EXIT_COMMAND: &str = "/exit";
const QUIT_COMMAND: &str = "/quit";
const COPY_COMMAND: &str = "/copy";
const CLEAR_COMMAND: &str = "/clear";
const SAVE_COMMAND: &str = "/save";

// List of all available commands for autocomplete
pub const COMMANDS: [&str; 6] = [
    HELP_COMMAND,
    EXIT_COMMAND,
    QUIT_COMMAND,
    COPY_COMMAND,
    CLEAR_COMMAND,
    SAVE_COMMAND,
];

// Chat session manager for interactive coding sessions
pub struct ChatSession {
    pub session: Session,
    pub last_response: String,
    pub use_openrouter: bool,
    pub model: String,
    pub temperature: f32,
}

impl ChatSession {
    // Create a new chat session
    pub fn new(name: String, use_openrouter: bool, model: Option<String>) -> Self {
        let model_name = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let provider = if use_openrouter { "openrouter".to_string() } else { "default".to_string() };

        Self {
            session: Session::new(name, model_name.clone(), provider),
            last_response: String::new(),
            use_openrouter,
            model: model_name,
            temperature: 0.7, // Default temperature
        }
    }

    // Initialize a new chat session or load existing one
    pub fn initialize(name: Option<String>, resume: Option<String>, use_openrouter: bool, model: String) -> Result<Self> {
        let sessions_dir = get_sessions_dir()?;

        // Determine session name
        let session_name = if let Some(name_arg) = &name {
            name_arg.clone()
        } else if let Some(resume_name) = &resume {
            resume_name.clone()
        } else {
            // Generate a name based on timestamp
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!("session_{}", timestamp)
        };

        let session_file = sessions_dir.join(format!("{}.jsonl", session_name));

        // Load or create session
        if (resume.is_some() || (name.is_some() && session_file.exists())) && session_file.exists() {
            println!("Resuming session: {}", session_name);
            let session = load_session(&session_file)?;

            // Create chat session from loaded session
            let mut chat_session = ChatSession {
                session,
                last_response: String::new(),
                use_openrouter: use_openrouter,
                model: model.clone(),
                temperature: 0.7,
            };

            // Get last assistant response if any
            for msg in chat_session.session.messages.iter().rev() {
                if msg.role == "assistant" {
                    chat_session.last_response = msg.content.clone();
                    break;
                }
            }

            Ok(chat_session)
        } else {
            // Create new session
            println!("Starting new session: {}", session_name);

            // Create session file if it doesn't exist
            if !session_file.exists() {
                let file = File::create(&session_file)?;
                drop(file);
            }

            let mut chat_session = ChatSession::new(session_name, use_openrouter, Some(model));
            chat_session.session.session_file = Some(session_file);

            Ok(chat_session)
        }
    }

    // Save the session
    pub fn save(&self) -> Result<()> {
        self.session.save()
    }

    // Add a system message
    pub fn add_system_message(&mut self, content: &str) -> Result<()> {
        // Add message to session
        self.session.add_message("system", content);

        // Save to session file
        if let Some(session_file) = &self.session.session_file {
            let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
            super::append_to_session_file(session_file, &format!("SYSTEM: {}", message_json))?;
        }

        Ok(())
    }

    // Add a user message
    pub fn add_user_message(&mut self, content: &str) -> Result<()> {
        // Add message to session
        self.session.add_message("user", content);

        // Save to session file
        if let Some(session_file) = &self.session.session_file {
            let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
            super::append_to_session_file(session_file, &format!("USER: {}", message_json))?;
        }

        Ok(())
    }

    // Add an assistant message
    pub fn add_assistant_message(&mut self, content: &str, exchange: Option<openrouter::OpenRouterExchange>) -> Result<()> {
        // Add message to session
        let message = self.session.add_message("assistant", content);
        self.last_response = content.to_string();

        // Save to session file
        if let Some(session_file) = &self.session.session_file {
            let message_json = serde_json::to_string(&message)?;
            super::append_to_session_file(session_file, &format!("ASSISTANT: {}", message_json))?;

            // If we have a raw exchange, save it as well
            if let Some(ex) = exchange {
                let exchange_json = serde_json::to_string(&ex)?;
                super::append_to_session_file(session_file, &format!("EXCHANGE: {}", exchange_json))?;
            }
        }

        Ok(())
    }

    // Process user commands
    pub fn process_command(&mut self, input: &str) -> Result<bool> {
        match input.trim() {
            EXIT_COMMAND | QUIT_COMMAND => {
                println!("Ending session. Your conversation has been saved.");
                return Ok(true);
            },
            HELP_COMMAND => {
                println!("\nAvailable commands:\n");
                println!("{} or {} - Exit the session", EXIT_COMMAND, QUIT_COMMAND);
                println!("{} - Copy last response to clipboard", COPY_COMMAND);
                println!("{} - Clear the screen", CLEAR_COMMAND);
                println!("{} - Save the session", SAVE_COMMAND);
                println!("{} - Show this help message\n", HELP_COMMAND);
            },
            COPY_COMMAND => {
                println!("Clipboard functionality is disabled in this version.");
            },
            CLEAR_COMMAND => {
                // ANSI escape code to clear screen and move cursor to top-left
                print!("\x1B[2J\x1B[1;1H");
                io::stdout().flush()?;
            },
            SAVE_COMMAND => {
                if let Err(e) = self.save() {
                    println!("Failed to save session: {}", e);
                } else {
                    println!("Session saved successfully.");
                }
            },
            _ => return Ok(false), // Not a command
        }

        Ok(false) // Continue session
    }
}

// Animation frames for loading indicator
const LOADING_FRAMES: [&str; 8] = [
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧",
];

// Read user input with support for multiline input and command completion
pub fn read_user_input() -> Result<String> {
    // Configure rustyline
    let config = RustylineConfig::builder()
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .auto_add_history(true) // Automatically add lines to history
        .bell_style(rustyline::config::BellStyle::None) // No bell
        .build();

    // Create editor with our custom helper
    let mut editor = Editor::with_config(config)?;

    // Add command completion
    use crate::session::chat_helper::CommandHelper;
    editor.set_helper(Some(CommandHelper::new()));

    // Set prompt
    let prompt = "> ";

    // Read line with command completion
    match editor.readline(prompt) {
        Ok(line) => {
            // Add to history
            let _ = editor.add_history_entry(line.clone());
            Ok(line)
        },
        Err(ReadlineError::Interrupted) => {
            // Ctrl+C
            println!("\nCancelled");
            Ok(String::new())
        },
        Err(ReadlineError::Eof) => {
            // Ctrl+D
            println!("\nExiting session.");
            Ok("/exit".to_string())
        },
        Err(err) => {
            println!("Error: {:?}", err);
            Ok(String::new())
        }
    }
}

// Show loading animation while waiting for response
async fn show_loading_animation(cancel_flag: Arc<AtomicBool>) -> Result<()> {
    let mut stdout = stdout();
    let mut frame_idx = 0;

    // Save cursor position
    execute!(stdout, cursor::SavePosition)?;

    while !cancel_flag.load(Ordering::SeqCst) {
        // Display frame
        execute!(stdout, cursor::RestorePosition)?;
        print!(" {} Generating response...", LOADING_FRAMES[frame_idx]);
        stdout.flush()?;

        // Update frame index
        frame_idx = (frame_idx + 1) % LOADING_FRAMES.len();

        // Delay
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    // Clear loading message
    execute!(stdout, cursor::RestorePosition)?;
    print!("                             "); // Clear loading message
    execute!(stdout, cursor::RestorePosition)?;
    stdout.flush()?;

    Ok(())
}

// Run an interactive session
pub async fn run_interactive_session<T: clap::Args + std::fmt::Debug>(
    args: &T,
    store: &Store,
    config: &Config,
) -> Result<()> {
    use clap::Args;
	use std::fmt::Debug;

    // Extract args from clap::Args
	#[derive(Args, Debug)]
    struct SessionArgs {
        /// Name of the session to start or resume
        #[arg(long, short)]
        name: Option<String>,

        /// Resume an existing session
        #[arg(long, short)]
        resume: Option<String>,

        /// Use OpenRouter model instead of default
        #[arg(long)]
        openrouter: bool,

        /// OpenRouter model to use
        #[arg(long, default_value = CLAUDE_MODEL)]
        model: String,
    }

    // Read args as SessionArgs
    let args_str = format!("{:?}", args);
    let session_args: SessionArgs = if args_str.contains("openrouter: true") {
        // Get model
        let model = if args_str.contains("model: \"") {
            let start = args_str.find("model: \"").unwrap() + 8;
            let end = args_str[start..].find('\"').unwrap() + start;
            args_str[start..end].to_string()
        } else {
            CLAUDE_MODEL.to_string()
        };

        // Get name
        let name = if args_str.contains("name: Some(\"") {
            let start = args_str.find("name: Some(\"").unwrap() + 12;
            let end = args_str[start..].find('\"').unwrap() + start;
            Some(args_str[start..end].to_string())
        } else {
            None
        };

        // Get resume
        let resume = if args_str.contains("resume: Some(\"") {
            let start = args_str.find("resume: Some(\"").unwrap() + 14;
            let end = args_str[start..].find('\"').unwrap() + start;
            Some(args_str[start..end].to_string())
        } else {
            None
        };

        SessionArgs {
            name,
            resume,
            openrouter: true,
            model,
        }
    } else {
        // Get name
        let name = if args_str.contains("name: Some(\"") {
            let start = args_str.find("name: Some(\"").unwrap() + 12;
            let end = args_str[start..].find('\"').unwrap() + start;
            Some(args_str[start..end].to_string())
        } else {
            None
        };

        // Get resume
        let resume = if args_str.contains("resume: Some(\"") {
            let start = args_str.find("resume: Some(\"").unwrap() + 14;
            let end = args_str[start..].find('\"').unwrap() + start;
            Some(args_str[start..end].to_string())
        } else {
            None
        };

        SessionArgs {
            name,
            resume,
            openrouter: false,
            model: CLAUDE_MODEL.to_string(),
        }
    };

    // Ensure there's an index
    let current_dir = std::env::current_dir()?;
    let octodev_dir = current_dir.join(".octodev");
    let index_path = octodev_dir.join("qdrant");
    if !index_path.exists() {
        println!("No index found. Indexing current directory first...");
        crate::indexer::index_files(store, crate::state::create_shared_state(), config).await?;
    }

    // Create or load session
    let mut chat_session = ChatSession::initialize(
        session_args.name,
        session_args.resume,
        session_args.openrouter,
        session_args.model
    )?;

    // Start the interactive session
    println!("Interactive coding session started. Type your questions/requests.");
    println!("Type /help for available commands.");

    // Initialize with system prompt if new session
    if chat_session.session.messages.is_empty() {
        // Create system prompt
        let system_prompt = create_system_prompt(&current_dir);
        chat_session.add_system_message(&system_prompt)?;

        // Add assistant welcome message
        let welcome_message = format!(
            "Hello! I'm ready to help you with your code in `{}`. What would you like to do?",
            current_dir.file_name().unwrap_or_default().to_string_lossy()
        );
        chat_session.add_assistant_message(&welcome_message, None)?;

        // Print welcome message
        println!("AI: {}", welcome_message);
    } else {
        // Print the last few messages for context
        let last_messages = chat_session.session.messages.iter().rev().take(3).collect::<Vec<_>>();
        for msg in last_messages.iter().rev() {
            if msg.role == "assistant" {
                println!("AI: {}", msg.content);
            } else if msg.role == "user" {
                println!("> {}", msg.content);
            }
        }
    }

    // Set up a shared cancellation flag that can be set by Ctrl+C
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_flag_clone = cancel_flag.clone();

    // Set up Ctrl+C handler
    ctrlc::set_handler(move || {
        cancel_flag_clone.store(true, Ordering::SeqCst);
    }).expect("Error setting Ctrl+C handler");

    // Main interaction loop
    loop {
        // Reset the cancel flag before each interaction
        cancel_flag.store(false, Ordering::SeqCst);

        // Read user input with command completion
        let input = read_user_input()?;

        // Check if the input is an exit command from Ctrl+D
        if input == "/exit" || input == "/quit" {
            println!("Ending session. Your conversation has been saved.");
            break;
        }

        // Skip if input is empty (could be from Ctrl+C)
        if input.trim().is_empty() {
            continue;
        }

        // Check if this is a command
        if input.starts_with('/') {
            let exit = chat_session.process_command(&input)?;
            if exit {
                break;
            }
            continue;
        }

        // Add user message
        chat_session.add_user_message(&input)?;

        // Process with OpenRouter
        if chat_session.use_openrouter {
            // Convert messages to OpenRouter format
            let or_messages = openrouter::convert_messages(&chat_session.session.messages);

            // Call OpenRouter in a separate task
            let model = chat_session.model.clone();
            let temperature = chat_session.temperature;
            let api_task = tokio::spawn(async move {
                openrouter::chat_completion(or_messages, &model, temperature).await
            });

            // Create a task to show loading animation
            let animation_cancel_flag = cancel_flag.clone();
            let animation_task = tokio::spawn(async move {
                let _ = show_loading_animation(animation_cancel_flag).await;
            });

            // Poll for completion or cancellation
            let mut response = None;
            let mut was_cancelled = false;

            tokio::select! {
                result = api_task => {
                    response = Some(result);
                },
                _ = async {
                    while !cancel_flag.load(Ordering::SeqCst) {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                } => {
                    was_cancelled = true;
                }
            }

            // Stop the animation
            cancel_flag.store(true, Ordering::SeqCst);
            let _ = animation_task.await;

            // Handle cancellation or response
            if was_cancelled {
                println!("\nRequest cancelled by user.");
                continue;
            }

            // Process the response
            match response.unwrap() {
                Ok(Ok((content, exchange))) => {
                    // Add assistant message with the exchange information
                    chat_session.add_assistant_message(&content, Some(exchange))?;

                    // Print assistant response
                    println!("\nAI: {}", content);
                },
                Ok(Err(e)) => {
                    println!("\nError calling OpenRouter: {}", e);
                    println!("Make sure OPENROUTER_API_KEY environment variable is set.");
                },
                Err(e) => {
                    println!("\nTask error: {}", e);
                }
            }
        } else {
            // Placeholder for other implementations
            let simulated_response = format!("No AI provider configured. Please use --openrouter flag.");
            chat_session.add_assistant_message(&simulated_response, None)?;
            println!("\nAI: {}", simulated_response);
        }
    }

    Ok(())
}
