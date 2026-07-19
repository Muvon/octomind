// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Import terminal output prelude to shadow std macros globally
// This automatically suspends the spinner before printing to prevent interference

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

use octomind::config::Config;

mod commands;

#[derive(Parser)]
#[command(name = "octomind")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Octomind is a smart AI developer assistant with configurable MCP support")]
struct CliArgs {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Subcommand)]
enum Commands {
	/// Generate a default configuration file
	Config(commands::ConfigArgs),

	/// Run an agent or start an interactive session.
	/// TAG can be a registry agent (e.g. `developer:general`) or a role name (e.g. `developer`).
	/// Use --format to run non-interactively.
	Run(commands::RunArgs),

	/// Sign in to your Octomind account — confirm a code in the browser and the CLI
	/// stores the hub key it mints.
	Login(commands::LoginArgs),

	/// Start WebSocket server for remote AI sessions
	Server(commands::ServerArgs),

	/// Run as an ACP (Agent Client Protocol) agent over stdio
	Acp(commands::AcpArgs),

	/// Add a registry tap (agent source URL).
	/// Omit URL to list all active taps.
	Tap(commands::TapArgs),

	/// Remove a previously added registry tap.
	Untap(commands::UntapArgs),

	/// Show all available placeholder variables and their values
	Vars(commands::VarsArgs),

	/// Send a message to a running session by name.
	Send(commands::SendArgs),

	/// Run a multi-step workflow by NAME (fetched from taps) or from a local TOML file.
	/// Omit NAME to list available tap workflows. Reads input from stdin; writes the
	/// final result to stdout; per-step progress, cost, and tokens to stderr.
	Workflow(commands::WorkflowArgs),

	/// Generate shell completion scripts
	Completion {
		/// The shell to generate completion for
		#[arg(value_enum)]
		shell: Shell,
	},

	/// Print completion candidates for a subcommand (used by shell completion scripts).
	/// Outputs one candidate per line to stdout.
	#[command(hide = true)]
	Complete(commands::CompleteArgs),
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
	// Initialize environment tracker before loading .env
	let _tracker = octomind::config::get_env_tracker();

	// Load .env file from current directory (if exists)
	// This will override existing environment variables with .env values
	if let Err(e) = octomind::config::get_env_tracker()
		.lock()
		.unwrap()
		.load_dotenv_override()
	{
		// Logging isn't configured this early, so log_debug! would vanish. A real
		// .env parse error (missing file is not an error) must reach the user.
		eprintln!("Warning: failed to load .env file: {e}");
	}

	// Seed the thread-local working directory with the real launch cwd immediately,
	// so get_thread_working_directory() never falls back to a wrong std::env::current_dir().
	let launch_cwd = std::env::current_dir().unwrap_or_default();
	octomind::mcp::set_session_working_directory(launch_cwd);

	let args = CliArgs::parse();

	// Set process/terminal title for long-running subcommands so they're
	// self-identifying in `ps` and terminal tabs. `Run` is handled later in
	// the session main loop once the session ID is known.
	match &args.command {
		Commands::Acp(a) => {
			// Include the session name so individual runs are identifiable in
			// `ps` (tap runs pass `--name tap-<role>-<id>`).
			let title = match a.name.as_deref() {
				Some(name) => format!("octomind-acp {name}"),
				None => "octomind-acp".to_string(),
			};
			octomind::proctitle::set_process_title(&title);
		}
		Commands::Server(_) => {
			octomind::proctitle::set_process_title("octomind-server");
			octomind::proctitle::set_terminal_title("octomind-server");
		}
		_ => {}
	}

	// Load configuration
	let config = Config::load()?;

	// Setup cleanup for MCP server processes when the program exits
	let result = run_with_cleanup(args, config).await;

	// Make sure to clean up any started server processes
	if let Err(e) = octomind::mcp::server::cleanup_servers() {
		octomind::log_error!("Warning: Error cleaning up MCP servers: {}", e);
	}

	result
}

async fn run_with_cleanup(args: CliArgs, config: Config) -> Result<(), anyhow::Error> {
	let log_level = config.log_level.as_str();
	if let Commands::Run(_) = &args.command {
		if let Err(e) = octomind::logging::tracing_setup::init_tracing(
			octomind::logging::tracing_setup::LoggingMode::Cli,
			log_level,
		) {
			eprintln!("Warning: Failed to initialize tracing: {e}");
		}
	}

	let sandbox_enabled = match &args.command {
		Commands::Run(a) => config.sandbox || a.sandbox,
		Commands::Server(a) => config.sandbox || a.sandbox,
		Commands::Acp(a) => config.sandbox || a.sandbox,
		_ => false,
	};
	if sandbox_enabled {
		let cwd = std::env::current_dir()?;
		octomind::sandbox::apply(&cwd)?;
	}

	match args.command {
		Commands::Config(config_args) => commands::config::execute(&config_args, config)?,
		Commands::Run(run_args) => commands::run::execute(&run_args, &config).await?,
		Commands::Login(login_args) => commands::login::execute(&login_args).await?,
		Commands::Server(server_args) => commands::server::execute(&server_args, &config).await?,
		Commands::Acp(acp_args) => commands::acp::execute(&acp_args, &config).await?,
		Commands::Tap(tap_args) => commands::tap::execute(&tap_args)?,
		Commands::Untap(untap_args) => commands::untap::execute(&untap_args)?,
		Commands::Vars(vars_args) => commands::vars::execute(&vars_args, &config).await?,
		Commands::Send(send_args) => commands::send::execute(&send_args).await?,
		Commands::Workflow(wf_args) => commands::workflow::execute(&wf_args, &config).await?,
		Commands::Completion { shell } => {
			let mut app = CliArgs::command();
			let name = app.get_name().to_string();
			let mut buf = Vec::new();
			generate(shell, &mut app, &name, &mut buf);
			let script = String::from_utf8_lossy(&buf);
			let patched = patch_completion_script(&script, shell);
			print!("{patched}");
		}
		Commands::Complete(complete_args) => commands::complete::execute(&complete_args, &config)?,
	}

	Ok(())
}

/// Patch the clap-generated completion script to add dynamic positional
/// completions, by calling `octomind complete <sub>` at runtime. Applied to
/// both `run` (agent tags + roles) and `workflow` (tap workflow names).
fn patch_completion_script(script: &str, shell: Shell) -> String {
	match shell {
		Shell::Bash => {
			let s = patch_bash(script, "run", "TAG");
			patch_bash(&s, "workflow", "NAME")
		}
		Shell::Zsh => {
			let s = patch_zsh(script, "run", "tag");
			patch_zsh(&s, "workflow", "name")
		}
		Shell::Fish => {
			let s = patch_fish(script, "run", "Agent tag or role name");
			patch_fish(&s, "workflow", "Workflow name")
		}
		// PowerShell and Elvish: emit as-is (no dynamic patching needed for now)
		_ => script.to_string(),
	}
}

/// Bash: patch the `octomind__<sub>)` block so its positional gets dynamic
/// completions from `octomind complete <sub>` instead of falling back to
/// file/directory completion. `value_name` is the positional's placeholder
/// token (`TAG`, `NAME`) that clap emits in the opts string.
///
/// Three problems in the clap-generated script that we fix here:
/// 1. Early-return fires at `COMP_CWORD -eq 2`, returning the literal
///    `[<VALUE_NAME>]` placeholder before the dynamic path is ever reached.
/// 2. The `*)` fallback branch has no `return 0`, so the result it sets is
///    immediately overwritten by the unconditional `COMPREPLY=…` after `esac`.
/// 3. The opts string contains the literal `[<VALUE_NAME>]` token which would
///    appear as a completion candidate when typing flags.
fn patch_bash(script: &str, sub: &str, value_name: &str) -> String {
	// The case label uses 8 spaces of indentation in the clap output.
	let marker = format!("        octomind__{sub})\n");
	let Some(run_pos) = script.find(&marker) else {
		return script.to_string();
	};
	let block_start = run_pos + marker.len();

	// Find the end of this block: next case label at the same indent level.
	let end_marker = "\n        octomind__";
	let block_len = script[block_start..]
		.find(end_marker)
		.unwrap_or(script.len() - block_start);
	let block_end = block_start + block_len;

	let block = &script[block_start..block_end];

	// Fix 1: remove the `|| ${COMP_CWORD} -eq 2` guard so typing
	// `octomind <sub> <TAB>` reaches the dynamic completion path.
	let block = block.replace(" || ${COMP_CWORD} -eq 2", "");

	// Fix 2: remove the literal `[<VALUE_NAME>]` placeholder from opts so it
	// never appears as a candidate when the user types a flag.
	let block = block.replace(&format!(" [{value_name}]"), "");

	// Fix 3: replace `COMPREPLY=()` in the `*)` branch with a dynamic call
	// and add `return 0` so the result is not overwritten after `esac`.
	let block = block.replace(
		"                    COMPREPLY=()\n                    ;;\n",
		&format!(
			"                    COMPREPLY=($(compgen -W \"$(octomind complete {sub} 2>/dev/null)\" -- \"${{cur}}\"))\n                    return 0\n                    ;;\n"
		),
	);

	format!(
		"{}{}{}{}",
		&script[..run_pos],
		marker,
		block,
		&script[block_end..]
	)
}

/// Zsh: inject a helper function and replace the `_default` completer on the
/// `<field_id>` positional argument inside the `(<sub>)` block. `field_id` is
/// the clap arg id as it appears in the generated descriptor (`tag`, `name`).
fn patch_zsh(script: &str, sub: &str, field_id: &str) -> String {
	// The helper must live in the file, but `#compdef octomind` MUST be the
	// very first line — zsh's compinit only reads line 1 to decide whether to
	// register the file as a completion. If anything appears before #compdef,
	// the file is silently ignored and completion falls back to files/dirs.
	//
	// Strategy: keep #compdef on line 1, then inject the helper right after it.
	//
	// Use compadd instead of _describe: _describe treats ':' as the
	// completion:description separator, which breaks tags like 'developer:general'.
	let helper = format!(
		"\n_octomind_complete_{sub}() {{\n  local -a items\n  items=(${{(f)\"$(octomind complete {sub} 2>/dev/null)\"}})\n  compadd -a items\n}}\n"
	);

	// Find the end of the first line (#compdef octomind).
	let after_first_line = script.find('\n').map(|i| i + 1).unwrap_or(script.len());
	let first_line = &script[..after_first_line];
	let rest = &script[after_first_line..];

	// Patch the positional completer in the (<sub>) block.
	let run_marker = format!("\n({sub})\n");
	let patched_rest = if let Some(run_start) = rest.find(&run_marker) {
		let block_body_start = run_start + run_marker.len();
		let block_body = &rest[block_body_start..];
		let block_len = block_body.find("\n(").unwrap_or(block_body.len());
		let run_block = &block_body[..block_len];

		let tag_prefix = format!("'::{field_id} -- ");
		let tag_suffix = ":_default' \\";
		if let (Some(tag_pos), Some(suffix_rel)) = (
			run_block.find(&tag_prefix),
			run_block
				.find(&tag_prefix)
				.and_then(|p| run_block[p..].find(tag_suffix)),
		) {
			let abs = block_body_start + tag_pos + suffix_rel;
			format!(
				"{}:_octomind_complete_{sub}' \\{}",
				&rest[..abs],
				&rest[abs + tag_suffix.len()..]
			)
		} else {
			rest.to_string()
		}
	} else {
		rest.to_string()
	};

	format!("{first_line}{helper}\n{patched_rest}")
}

/// Fish: append a dynamic completion line for `octomind <sub>`'s positional.
/// Fish doesn't emit a positional-arg slot, so we append a line that calls
/// `octomind complete <sub>` as the candidate source. `desc` is the candidate
/// description shown by fish.
fn patch_fish(script: &str, sub: &str, desc: &str) -> String {
	let dynamic_line = format!(
		"\n# Dynamic positional completions for `octomind {sub}`\ncomplete -c octomind -n '__fish_octomind_using_subcommand {sub}' -f -a '(octomind complete {sub} 2>/dev/null)' -d '{desc}'\n"
	);
	format!("{script}{dynamic_line}")
}
