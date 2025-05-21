use crate::session::Session;
use crate::session::project_context::ProjectContext;
use std::collections::HashMap;
use std::path::Path;

// Function to get a system prompt for a specific layer by string type name
pub fn get_layer_system_prompt_for_type(layer_type: &str) -> String {
	// Get the raw system prompt without any substitutions
	let raw_prompt = get_raw_system_prompt(layer_type);

	// For now, we'll return the raw prompt. The placeholder substitution will be done
	// by process_placeholders when the prompt is actually used
	raw_prompt
}

// Function to get the raw system prompt without any substitutions
pub fn get_raw_system_prompt(layer_type: &str) -> String {
	match layer_type {
		"query_processor" => format!("You are an expert query processor and requirement analyst in the OctoDev system. \
			Your only job is to analyze the user's request and return an improved, clarified version of the task. \
			Transform vague or ambiguous requests into specific, actionable instructions. \
			Identify unstated requirements, technical constraints, and implementation details that would be needed. \
			Structure the output as a clear set of development tasks or requirements. \
			Include relevant technical specifics, edge cases to handle, and success criteria when possible. \
			DO NOT use tools or explore the codebase - that will be done in a later stage. \
			Return only the refined task description that clearly explains what needs to be done.%{{CONTEXT}}"),

		"context_generator" => format!("You are the context gathering specialist for the OctoDev system. \
			\
			Your primary responsibilities are to: \
			1. Take the original query and the improved instructions from the query processor \
			2. Identify ALL information needed for task resolution \
			3. Methodically gather relevant context through available tools \
			4. Construct a comprehensive context package that will be provided to the developer \
			\
			CONTEXT IDENTIFICATION PROCESS: \
			- Determine the programming language, frameworks, and technologies involved \
			- Identify relevant files, classes, functions, configurations, or dependencies \
			- Consider what implementation patterns or architectural decisions may impact the solution \
			- Assess if environment configuration, build settings, or runtime details are relevant \
			\
			INFORMATION GATHERING GUIDELINES: \
			- USE TOOLS to explore the codebase and gather information \
			- Always check for existing implementations of similar functionality in the codebase \
			- Retrieve complete file contents when structure or relationships are important \
			- For large files, focus on the most relevant sections (class definitions, function signatures) \
			- Collect documentation, READMEs, or comments that explain design decisions \
			- When imports or dependencies are referenced, fetch their definitions if needed \
			\
			Your output should be a well-organized collection of context information that the developer can use to solve the task. \
			Begin your response with the refined task from the query processor, then include all the relevant context you've gathered.%{{CONTEXT}}"),

		"developer" => format!("You are an Octodev – top notch fully autonomous AI developer.\n\
			Current working dir: %{{CWD}}\n\
			**DEVELOPMENT APPROACH:**\n\
			1. Analyze problems thoroughly first\n\
			2. Think through solutions step-by-step\n\
			3. Execute necessary changes directly using available tools\n\
			4. Test your implementations when possible\n\n\
			**CODE QUALITY GUIDELINES:**\n\
			• Provide validated, working solutions\n\
			• Keep code clear and concise\n\
			• Focus on practical solutions and industry best practices\n\
			• Avoid unnecessary abstractions - solve problems directly\n\
			• Balance file size and readability\n\
			• Don't over-fragment code across multiple files\n\n\
			**WHEN WORKING WITH FILES:**\n\
			1. First understand which files you need to read/write\n\
			2. Process files efficiently, preferably in a single operation\n\
			3. Utilize the provided tools proactively without asking if you should use them\n\n\
			%{{CONTEXT}}\n\
			Right now you are *NOT* in the chat only mode and have access to tool use and system."),
		"reducer" => format!("You are the session optimizer for OctoDev, responsible for consolidating information and preparing for the next interaction. \
			\
			Your responsibilities: \
			1. Review the original request and the developer's solution \
			2. Ensure documentation (README.md and CHANGES.md) is properly updated \
			3. Create a concise summary of the work that was done \
			4. Condense the context in a way that preserves essential information for future requests \
			\
			This condensed information will be cached to reduce token usage in the next iteration. \
			Focus on extracting the most important technical details while removing unnecessary verbosity. \
			Your output will be used as context for the next user interaction, so it must contain all essential information \
			while being as concise as possible.%{{CONTEXT}}"),

		_ => format!("You are the {} layer in the OctoDev system.%{{CONTEXT}}", layer_type),
	}
}

// Function to process placeholders in a system prompt
pub fn process_placeholders(prompt: &str, project_dir: &Path) -> String {
	let mut processed_prompt = prompt.to_string();

	// Collect context information
	let project_context = ProjectContext::collect(project_dir);

	// Create a map of placeholder values
	let mut placeholders = HashMap::new();

	// Context section format
	let context_info = project_context.format_for_prompt();
	let context_section = if !context_info.is_empty() {
		format!("\n\n==== PROJECT CONTEXT ====\n\n{}\n\n==== END PROJECT CONTEXT ====\n", context_info)
	} else {
		String::new()
	};

	// Add the placeholders
	placeholders.insert("%{CONTEXT}", context_section);

	placeholders.insert("%{CWD}", project_dir.to_string_lossy().to_string());

	// Add specific parts of the context as individual placeholders
	placeholders.insert("%{GIT_STATUS}", if let Some(git_status) = &project_context.git_status {
		format!("\n\n==== GIT STATUS ====\n\n{}\n\n==== END GIT STATUS ====\n", git_status)
	} else {
			String::new()
		});

	placeholders.insert("%{GIT_TREE}", if let Some(file_tree) = &project_context.file_tree {
		format!("\n\n==== FILE TREE ====\n\n{}\n\n==== END FILE TREE ====\n", file_tree)
	} else {
			String::new()
		});

	placeholders.insert("%{README}", if let Some(readme) = &project_context.readme_content {
		format!("\n\n==== README ====\n\n{}\n\n==== END README ====\n", readme)
	} else {
			String::new()
		});

	// Replace all placeholders
	for (placeholder, value) in placeholders.iter() {
		processed_prompt = processed_prompt.replace(placeholder, value);
	}

	processed_prompt
}

// Function to get summarized context for layers using the Summary InputMode
pub fn summarize_context(session: &Session, input: &str) -> String {
	// This is a placeholder. In practice, you'd want to analyze the session history
	// and create a summary of the important points rather than just concatenating everything.
	let history = session.messages.iter()
		.filter(|m| m.role == "assistant")
		.map(|m| m.content.clone())
		.collect::<Vec<_>>()
		.join("\n\n");

	format!("Current input: {}\n\nSummary of previous context: {}\n\nPlease generate a concise summary of the above context.",
		input,
		if history.len() > 2000 {
			format!("{} (truncated)...", &history[..2000])
		} else {
			history
		}
	)
}
