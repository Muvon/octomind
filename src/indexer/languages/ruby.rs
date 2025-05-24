//! Ruby language implementation for the indexer

use tree_sitter::Node;
use crate::indexer::languages::Language;

pub struct Ruby {}

impl Language for Ruby {
	fn name(&self) -> &'static str {
		"ruby"
	}

	fn get_ts_language(&self) -> tree_sitter::Language {
		tree_sitter_ruby::LANGUAGE.into()
	}

	fn get_meaningful_kinds(&self) -> Vec<&'static str> {
		vec![
			"method",
			"class",
			"module",
		]
	}

	fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String> {
		let mut symbols = Vec::new();

		match node.kind() {
			"method" | "class" | "module" => {
				// Find method, class, or module name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "identifier" || child.kind() == "constant" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
					}
				}

				// For methods, extract local variables
				if node.kind() == "method" {
					for child in node.children(&mut node.walk()) {
						if child.kind() == "body_statement" || child.kind() == "do_block" {
							self.extract_ruby_variables(child, contents, &mut symbols);
							break;
						}
					}
				}
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
		// Check if this is a valid identifier or constant
		if kind == "identifier" || kind == "constant" {
			if let Ok(text) = node.utf8_text(contents.as_bytes()) {
				let t = text.trim();
				if !t.is_empty() && !symbols.contains(&t.to_string()) && !t.starts_with('@') {
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

impl Ruby {
	/// Extract local variable assignments in Ruby
	#[allow(clippy::only_used_in_recursion)]
	fn extract_ruby_variables(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				if child.kind() == "assignment" {
					// Extract variable name from assignment
					for assign_child in child.children(&mut child.walk()) {
						if assign_child.kind() == "identifier" {
							if let Ok(name) = assign_child.utf8_text(contents.as_bytes()) {
								// Skip instance/class variables (starting with @ or @@)
								if !name.starts_with('@') && !symbols.contains(&name.to_string()) {
									symbols.push(name.to_string());
								}
							}
							break;  // Only take the left side (the variable name)
						}
					}
				} else {
					// Recursive search in nested structures
					self.extract_ruby_variables(child, contents, symbols);
				}

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}
