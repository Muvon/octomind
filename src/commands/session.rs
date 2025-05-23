use clap::Args;

#[derive(Args, Debug)]
pub struct SessionArgs {
	/// Name of the session to start or resume
	#[arg(long, short)]
	pub name: Option<String>,

	/// Resume an existing session
	#[arg(long, short)]
	pub resume: Option<String>,

	/// Use a specific model instead of the one configured in config
	#[arg(long)]
	pub model: Option<String>,

	/// Session mode: agent (default with layers and tools) or chat (simple chat without tools)
	#[arg(long, default_value = "agent")]
	pub mode: String,
}

// No execute function here since it's handled directly by the session::chat module
// The module is accessed in main.rs via:
// session::chat::run_interactive_session(session_args, &store, &config).await?
