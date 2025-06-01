use crate::session::Session;
use crate::session::project_context::ProjectContext;
use std::collections::HashMap;
use std::path::Path;

// Function to get a system prompt for a specific layer by string type name
pub fn get_layer_system_prompt_for_type(layer_type: &str) -> String {
	// Get the raw system prompt without any substitutions


	// For now, we'll return the raw prompt. The placeholder substitution will be done
	// by process_placeholders when the prompt is actually used
	get_raw_system_prompt(layer_type)
}

// Function to get the raw system prompt without any substitutions
pub fn get_raw_system_prompt(layer_type: &str) -> String {
	match layer_type {
		"query_processor" => "You are an expert query processor and requirement analyst in the Octodev system. \
			Your task is to analyze user requests and transform them into clearer, more actionable forms.\
			\
			Given a user request:\
			1. Identify the core requirement and intent\
			2. Structure and refine the request while preserving its fundamental purpose\
			3. Clarify ambiguities and add helpful technical specifics\
			4. Format the output as well-structured development tasks/requirements\
			5. Include relevant edge cases, constraints, and success criteria\
			\
			Guidelines:\
			- Make minimal changes if the request is already clear and specific\
			- Return the original text if the request cannot be understood\
			- Focus solely on requirement analysis - do not implement solutions or write code\
			- Return only the refined task description\
			- If you lack of context or do not understand it, keep original request unchanged\
			\
			%{CONTEXT}".to_string(),
		"context_generator" => "You are a context gathering specialist for development tasks.\
			\
			When given a new task, help me understand what I need to know before implementing it by:\
			\
			- First: Look into file signatures with semantic_code tool and try to analyze project structure related to task\
			- Then: If needed, use list_files to find relevant implementation patterns \
			- If needed: Use text_editor view to examine files and understand interfaces and code signatures \
			- Only when necessary: Look at detailed implementations\
			\
			For each task type, focus on different aspects:\
			- Configuration tasks: Config files, env settings, build scripts\
			- Feature implementation: Related modules, interfaces, patterns\
			- Bug fixes: Affected components and dependencies\
			- Refactoring: Impacted modules and relationships\
			\
			Provide a clear summary with:\
			- Core task requirements decomposited in the way you are project manager who made it\
			- Recommendations to look into list of given fields needing examination (with reasons)\
			- Key code structures and patterns found\
			- Potential implementation challenges\
			- Areas where more information might help\
			\
			Your goal is helping me fully understand what's needed to implement the task successfully.\
			\
			%{CONTEXT}".to_string(),
		"developer" => "You are an Octodev – top notch fully autonomous AI developer.\n\
			Current working dir: %{CWD}\n\
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
			**MISSING CONTEXT COLLECTION CHECKLIST:**\n\
			1. Examine key project files to understand the codebase structure \
			2. Use text_editor view to examine files and understand interfaces and code signatures \
			2. If needed, use list_files to find relevant implementation patterns \
			3. As a last resort, use text_editor to view specific file contents \
			**WHEN WORKING WITH FILES:**\n\
			1. First understand which files you need to read/write\n\
			2. Process files efficiently, preferably in a single operation\n\
			3. Utilize the provided tools proactively without asking if you should use them\n\n\
			%{CONTEXT}\n\
			\
			IMPORTANT:\n\
			- Right now you are *NOT* in the chat only mode and have access to tool use and system.\
			- Please follow the task provided and make sure you do only changes required by the task, if you found something outside of task scope, you can mention it and ask.\
			- Make sure when you refactor code or do changes, you do not remove critical parts of the codebase.\
			".to_string(),
		"reducer" => "You are the session optimizer for Octodev, responsible for consolidating information and preparing for the next interaction. \
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
			while being as concise as possible.%{CONTEXT}".to_string(),

		_ => format!("You are the {} layer in the Octodev system.%{{CONTEXT}}", layer_type),
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
