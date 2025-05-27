use clap::Args;

#[derive(Args, Debug)]
pub struct SessionArgs {
	/// Name of the session to start or resume
	#[arg(long, short)]
	pub name: Option<String>,

	/// Resume an existing session
	#[arg(long, short)]
	pub resume: Option<String>,

	/// Use a specific model instead of the one configured in config (runtime only, not saved)
	#[arg(long)]
	pub model: Option<String>,

	/// Temperature for the AI response (0.0 to 1.0, runtime only, not saved)
	#[arg(long, default_value = "0.7")]
	pub temperature: f32,

	/// Session role: developer (default with layers and tools) or assistant (simple chat without tools)
	#[arg(long, default_value = "developer")]
	pub role: String,
}

// No execute function here since it's handled directly by the session::chat module
// The module is accessed in main.rs via:
// session::chat::run_interactive_session(session_args, &store, &config).await?
