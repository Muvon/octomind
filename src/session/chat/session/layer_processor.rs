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

// Pipeline pre-processing — runs a role's configured deterministic
// pipeline (if any) on the raw user input before the main model sees it.
//
// In-session AI workflows were removed; multi-step AI orchestration now
// lives in the external `octomind workflow <file.toml>` command.

use super::core::ChatSession;
use crate::config::Config;
use crate::log_info;
use crate::session::pipelines::PipelineOrchestrator;
use anyhow::Result;
use colored::*;
use tokio::sync::watch;

/// Run the role's pipeline (if any) on `input` and return
/// `(processed_input, cancelled)`. Pipelines never mutate the chat session.
pub async fn process_pipeline_if_enabled(
	input: &str,
	_chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	first_message_processed: bool,
	operation_rx: watch::Receiver<bool>,
) -> Result<(String, bool)> {
	let pipeline_name = match config.role_map.get(role).and_then(|r| r.pipeline.as_ref()) {
		Some(name) if !first_message_processed => name.clone(),
		_ => return Ok((input.to_string(), false)),
	};

	let pipeline_def = config
		.pipelines
		.iter()
		.find(|p| p.name == pipeline_name)
		.ok_or_else(|| anyhow::anyhow!("Pipeline '{}' not found", pipeline_name))?
		.clone();

	let working_dir = config.get_working_directory();
	let orchestrator = PipelineOrchestrator::new(pipeline_def, pipeline_name.clone());

	log_info!("Running pipeline '{}'", pipeline_name);

	match orchestrator
		.execute(input, &working_dir, role, operation_rx)
		.await
	{
		Ok(output) => {
			log_info!("Pipeline '{}' completed.", pipeline_name);
			Ok((output, false))
		}
		Err(e) => {
			if crate::session::cancellation::is_cancelled(&e) {
				crate::log_debug!("Pipeline cancelled by user.");
				println!("{}", "Pipeline cancelled.".yellow());
				return Ok((input.to_string(), true));
			}
			// Pipeline errors are fatal — non-zero exit code = hard stop
			println!("\n{}: {}", "Pipeline failed".bright_red(), e);
			Err(e)
		}
	}
}
