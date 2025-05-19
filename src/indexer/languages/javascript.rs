//! JavaScript language implementation for the indexer

use tree_sitter::Node;
use crate::indexer::languages::Language;

pub struct JavaScript {}

impl Language for JavaScript {
	fn name(&self) -> &'static str {
		"javascript"
	}

	fn get_ts_language(&self) -> tree_sitter::Language {
		tree_sitter_javascript::LANGUAGE.into()
	}

	fn get_meaningful_kinds(&self) -> Vec<&'static str> {
		vec![
			"function_declaration",
			"method_definition",
			"class_declaration",
			"arrow_function",
		]
	}

	fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String> {
		let mut symbols = Vec::new();

		match node.kind() {
			"function_declaration" | "method_definition" | "class_declaration" => {
				// Extract name of the function, method or class
				for child in node.children(&mut node.walk()) {
					if child.kind() == "identifier" || child.kind().contains("name") {
						if let Ok(n) = child.utf8_text(contents.as_bytes()) {
							symbols.push(n.to_string());
						}
						break;
					}
				}

				// For JavaScript, look for variable declarations within the function/method body
				for child in node.children(&mut node.walk()) {
					if child.kind() == "statement_block" {
						self.extract_js_variable_declarations(child, contents, &mut symbols);
						break;
					}
				}
			}
			"arrow_function" => {
				// Extract parent variable name for arrow functions
				if let Some(parent) = node.parent() {
					if parent.kind() == "variable_declarator" {
						for child in parent.children(&mut parent.walk()) {
							if child.kind() == "identifier" {
								if let Ok(n) = child.utf8_text(contents.as_bytes()) {
									symbols.push(n.to_string());
								}
								break;
							}
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
		// Check if this is a valid identifier and not a property identifier
		if (kind.contains("identifier") || kind.contains("name")) && kind != "property_identifier" {
			if let Ok(text) = node.utf8_text(contents.as_bytes()) {
				let t = text.trim();
				if !t.is_empty() && !symbols.contains(&t.to_string()) {
					symbols.push(t.to_string());
				}
			}
		}

		// For JavaScript avoid excessive recursion into certain nodes
		// that tend to duplicate identifiers
		if node.kind() == "member_expression" || node.kind() == "property_access_expression" {
			// For member expressions, only take the object part (leftmost identifier)
			let mut cursor = node.walk();
			if cursor.goto_first_child() {
				// Process just the first child (object)
				self.extract_identifiers(cursor.node(), contents, symbols);
				return;
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

impl JavaScript {
	/// Extract JavaScript variable declarations within a block
	pub fn extract_js_variable_declarations(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		// Look through all children
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();
				// Look for variable or lexical declarations
				if child.kind() == "variable_declaration" || child.kind() == "lexical_declaration" {
					// For each declarator in the declaration
					for var_decl in child.children(&mut child.walk()) {
						if var_decl.kind() == "variable_declarator" {
							// Get the identifier from the declarator
							for decl_child in var_decl.children(&mut var_decl.walk()) {
								if decl_child.kind() == "identifier" {
									if let Ok(name) = decl_child.utf8_text(contents.as_bytes()) {
										let t = name.trim();
										if !t.is_empty() && !symbols.contains(&t.to_string()) {
											symbols.push(t.to_string());
										}
									}
									break; // Only take the first identifier (the variable name)
								}
							}
						}
					}
				}
				// Recursive search in nested blocks (if, for, while loops, etc.)
				else if child.kind() == "statement_block" ||
				child.kind().contains("statement") {
					self.extract_js_variable_declarations(child, contents, symbols);
				}

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}
