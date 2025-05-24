//! Go language implementation for the indexer

use tree_sitter::Node;
use crate::indexer::languages::Language;

pub struct Go {}

impl Language for Go {
	fn name(&self) -> &'static str {
		"go"
	}

	fn get_ts_language(&self) -> tree_sitter::Language {
		tree_sitter_go::LANGUAGE.into()
	}

	fn get_meaningful_kinds(&self) -> Vec<&'static str> {
		vec![
			"function_declaration",
			"method_declaration",
			"type_declaration",
			"struct_type",
			"interface_type",
		]
	}

	fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String> {
		let mut symbols = Vec::new();

		match node.kind() {
			"function_declaration" | "method_declaration" => {
				// Extract function or method name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "identifier" || child.kind() == "field_identifier" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
					}
				}

				// Extract variables declared in function body
				for child in node.children(&mut node.walk()) {
					if child.kind() == "block" {
						self.extract_go_variables(child, contents, &mut symbols);
						break;
					}
				}
			},
			"type_declaration" => {
				// Extract type name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "type_spec" {
						for type_child in child.children(&mut child.walk()) {
							if type_child.kind() == "identifier" {
								if let Ok(name) = type_child.utf8_text(contents.as_bytes()) {
									symbols.push(name.to_string());
								}
								break;
							}
						}
					}
				}
			},
			"struct_type" | "interface_type" => {
				// Extract field names within structs or interfaces
				self.extract_struct_interface_fields(node, contents, &mut symbols);
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
		// Check if this is a valid identifier
		if kind == "identifier" || kind == "field_identifier" {
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

impl Go {
	/// Extract variable declarations in Go blocks
	#[allow(clippy::only_used_in_recursion)]
	fn extract_go_variables(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		// Traverse the block looking for variable declarations
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				match child.kind() {
					"short_var_declaration" => {
						// Handle short variables like x := 10
						for var_child in child.children(&mut child.walk()) {
							if var_child.kind() == "expression_list" {
								for expr in var_child.children(&mut var_child.walk()) {
									if expr.kind() == "identifier" {
										if let Ok(name) = expr.utf8_text(contents.as_bytes()) {
											if !symbols.contains(&name.to_string()) {
												symbols.push(name.to_string());
											}
										}
									}
								}
								break; // Only process the left side of :=
							}
						}
					},
					"var_declaration" => {
						// Handle var x = 10 or var x int = 10
						for spec in child.children(&mut child.walk()) {
							if spec.kind() == "var_spec" {
								for spec_child in spec.children(&mut spec.walk()) {
									if spec_child.kind() == "identifier" {
										if let Ok(name) = spec_child.utf8_text(contents.as_bytes()) {
											if !symbols.contains(&name.to_string()) {
												symbols.push(name.to_string());
											}
										}
									}
								}
							}
						}
					},
					"const_declaration" => {
						// Handle const declarations
						for spec in child.children(&mut child.walk()) {
							if spec.kind() == "const_spec" {
								for spec_child in spec.children(&mut spec.walk()) {
									if spec_child.kind() == "identifier" {
										if let Ok(name) = spec_child.utf8_text(contents.as_bytes()) {
											if !symbols.contains(&name.to_string()) {
												symbols.push(name.to_string());
											}
										}
									}
								}
							}
						}
					},
					"block" => {
						// Recursively process nested blocks
						self.extract_go_variables(child, contents, symbols);
					},
					"if_statement" | "for_statement" | "switch_statement" => {
						// Process blocks inside control structures
						for stmt_child in child.children(&mut child.walk()) {
							if stmt_child.kind() == "block" {
								self.extract_go_variables(stmt_child, contents, symbols);
							}
						}
					},
					_ => {}
				}

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}

	/// Extract field names from struct or interface types
	fn extract_struct_interface_fields(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				if child.kind() == "field_declaration" {
					for field_child in child.children(&mut child.walk()) {
						if field_child.kind() == "field_identifier" {
							if let Ok(name) = field_child.utf8_text(contents.as_bytes()) {
								if !symbols.contains(&name.to_string()) {
									symbols.push(name.to_string());
								}
							}
						}
					}
				} else if child.kind() == "method_spec" {
					// For interface methods
					for method_child in child.children(&mut child.walk()) {
						if method_child.kind() == "field_identifier" {
							if let Ok(name) = method_child.utf8_text(contents.as_bytes()) {
								if !symbols.contains(&name.to_string()) {
									symbols.push(name.to_string());
								}
							}
						}
					}
				}

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}
}
