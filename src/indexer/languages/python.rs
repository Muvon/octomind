//! Python language implementation for the indexer

use tree_sitter::Node;
use crate::indexer::languages::Language;

pub struct Python {}

impl Language for Python {
	fn name(&self) -> &'static str {
		"python"
	}

	fn get_ts_language(&self) -> tree_sitter::Language {
		tree_sitter_python::LANGUAGE.into()
	}

	fn get_meaningful_kinds(&self) -> Vec<&'static str> {
		vec![
			"function_definition",
			"class_definition",
		]
	}

	fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String> {
		let mut symbols = Vec::new();

		match node.kind() {
			"function_definition" | "class_definition" => {
				// Find the identifier (name) node for the function or class
				for child in node.children(&mut node.walk()) {
					if child.kind() == "identifier" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
					}
				}

				// If it's a function, extract variable assignments within it
				if node.kind() == "function_definition" {
					for child in node.children(&mut node.walk()) {
						if child.kind() == "block" {
							self.extract_python_variables(child, contents, &mut symbols);
							break;
						}
					}
				}
			}
			_ => self.extract_identifiers(node, contents, &mut symbols),
		}

		// Deduplicate symbols before returning
		symbols.sort();
		symbols.dedup();

		symbols
	}

	fn extract_identifiers(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let kind = node.kind();
		// Check if this is a valid identifier
		if kind == "identifier" {
			if let Ok(text) = node.utf8_text(contents.as_bytes()) {
				let t = text.trim();
				if !t.is_empty() && !symbols.contains(&t.to_string()) && !t.starts_with("_") {
					symbols.push(t.to_string());
				}
			}
		}

		// Continue with normal recursion for other nodes
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				self.extract_identifiers(cursor.node(), contents, symbols);
				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}

impl Python {
	/// Extract variable assignments in Python blocks
	fn extract_python_variables(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				// Handle different types of assignments
				match child.kind() {
					"assignment" => {
						// For regular assignments like 'x = 10'
						let mut child_cursor = child.walk();
						if child_cursor.goto_first_child() {  // First child is the target
							let target = child_cursor.node();
							if target.kind() == "identifier" {
								if let Ok(name) = target.utf8_text(contents.as_bytes()) {
									if !name.starts_with("_") && !symbols.contains(&name.to_string()) {
										symbols.push(name.to_string());
									}
								}
							}
						}
					},
					"expression_statement" => {
						// Check for augmented assignments like 'x += 1'
						for expr_child in child.children(&mut child.walk()) {
							if expr_child.kind() == "augmented_assignment" {
								let mut aug_cursor = expr_child.walk();
								if aug_cursor.goto_first_child() {  // First child is target
									let target = aug_cursor.node();
									if target.kind() == "identifier" {
										if let Ok(name) = target.utf8_text(contents.as_bytes()) {
											if !name.starts_with("_") && !symbols.contains(&name.to_string()) {
												symbols.push(name.to_string());
											}
										}
									}
								}
							}
						}
					},
					"for_statement" | "while_statement" | "if_statement" | "try_statement" | "with_statement" => {
						// Recursive search in nested blocks
						for stmt_child in child.children(&mut child.walk()) {
							if stmt_child.kind() == "block" {
								self.extract_python_variables(stmt_child, contents, symbols);
							}
						}
					},
					_ => {}
				}

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}
