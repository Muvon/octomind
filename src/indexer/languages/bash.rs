//! Bash language implementation for the indexer

use tree_sitter::Node;
use crate::indexer::languages::Language;

pub struct Bash {}

impl Language for Bash {
	fn name(&self) -> &'static str {
		"bash"
	}

	fn get_ts_language(&self) -> tree_sitter::Language {
		tree_sitter_bash::LANGUAGE.into()
	}

	fn get_meaningful_kinds(&self) -> Vec<&'static str> {
		vec![
			"function_definition",
		]
	}

	fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String> {
		let mut symbols = Vec::new();

		match node.kind() {
			"function_definition" => {
				// Find the function name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "name" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
					}
				}

				// Extract variables from function body
				self.extract_bash_variables(node, contents, &mut symbols);
			},
			_ => self.extract_identifiers(node, contents, &mut symbols),
		}

		// Deduplicate symbols before returning
		symbols.sort();
		symbols.dedup();

		symbols
	}

	fn extract_identifiers(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let kind = node.kind();
		// Check if this is a variable name or command name
		if kind == "variable_name" || kind == "command_name" {
			if let Ok(text) = node.utf8_text(contents.as_bytes()) {
				let t = text.trim();
				if !t.is_empty() && !symbols.contains(&t.to_string()) {
					symbols.push(t.to_string());
				}
			}
		}

		// Continue with recursive traversal
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				self.extract_identifiers(cursor.node(), contents, symbols);
				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}

impl Bash {
	/// Extract variable assignments in bash
	fn extract_bash_variables(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				match child.kind() {
					"variable_assignment" => {
						for var_child in child.children(&mut child.walk()) {
							if var_child.kind() == "variable_name" {
								if let Ok(name) = var_child.utf8_text(contents.as_bytes()) {
									if !symbols.contains(&name.to_string()) {
										symbols.push(name.to_string());
									}
								}
								break;
							}
						}
					},
					_ => {}
				}

				// Continue traversing for nested assignments
				self.extract_identifiers(child, contents, symbols);

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}
