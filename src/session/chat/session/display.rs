// Session display functionality

use super::core::ChatSession;
use colored::*;

impl ChatSession {
	// Display detailed information about the session, including layer-specific stats
	pub fn display_session_info(&self) {
		// Display overall session metrics
		println!("{}", "───────────── Session Information ─────────────".bright_cyan());

		// Session basics
		println!("{} {}", "Session name:".yellow(), self.session.info.name.bright_white());
		println!("{} {}", "Main model:".yellow(), self.session.info.model.bright_white());

		// Total token usage
		let total_tokens = self.session.info.input_tokens + self.session.info.output_tokens + self.session.info.cached_tokens;
		println!("{} {}", "Total tokens:".yellow(), total_tokens.to_string().bright_white());
		println!("{} {} input, {} output, {} cached",
			"Breakdown:".yellow(),
			self.session.info.input_tokens.to_string().bright_blue(),
			self.session.info.output_tokens.to_string().bright_green(),
			self.session.info.cached_tokens.to_string().bright_magenta());

		// Cost information
		println!("{} ${:.5}", "Total cost:".yellow(), self.session.info.total_cost);

		// Messages count
		println!("{} {}", "Messages:".yellow(), self.session.messages.len());

		// Display layered stats if available
		if !self.session.info.layer_stats.is_empty() {
			println!();
			println!("{}", "───────────── Layer-by-Layer Statistics ─────────────".bright_cyan());

			// Group by layer type
			let mut layer_stats: std::collections::HashMap<String, Vec<&crate::session::LayerStats>> = std::collections::HashMap::new();

			// Group stats by layer type
			for stat in &self.session.info.layer_stats {
				layer_stats.entry(stat.layer_type.clone())
					.or_insert_with(Vec::new)
					.push(stat);
			}

			// Print stats for each layer type
			for (layer_type, stats) in layer_stats.iter() {
				// Add special highlighting for context optimization
				let layer_display = if layer_type == "context_optimization" {
					format!("Layer: {}", layer_type).bright_magenta()
				} else {
					format!("Layer: {}", layer_type).bright_yellow()
				};

				println!("{}", layer_display);

				// Count total tokens and cost for this layer type
				let mut total_input = 0;
				let mut total_output = 0;
				let mut total_cost = 0.0;

				// Count executions
				let executions = stats.len();

				for stat in stats {
					total_input += stat.input_tokens;
					total_output += stat.output_tokens;
					total_cost += stat.cost;
				}

				// Print the stats
				println!("  {}: {}", "Model".blue(), stats[0].model);
				println!("  {}: {}", "Executions".blue(), executions);
				println!("  {}: {} input, {} output",
					"Tokens".blue(),
					total_input.to_string().bright_white(),
					total_output.to_string().bright_white());
				println!("  {}: ${:.5}", "Cost".blue(), total_cost);

				// Add special note for context optimization
				if layer_type == "context_optimization" {
					println!("  {}", "Note: These are costs for optimizing context between interactions".bright_cyan());
				}

				println!();
			}
		} else {
			println!();
			println!("{}", "No layer-specific statistics available.".bright_yellow());
			println!("{}", "This may be because the session was created before layered architecture was enabled.".bright_yellow());
		}

		println!();
	}
}