//! C++ language implementation for the indexer

use tree_sitter::Node;
use crate::indexer::languages::Language;

pub struct Cpp {}

impl Language for Cpp {
	fn name(&self) -> &'static str {
		"cpp"
	}

	fn get_ts_language(&self) -> tree_sitter::Language {
		tree_sitter_cpp::LANGUAGE.into()
	}

	fn get_meaningful_kinds(&self) -> Vec<&'static str> {
		vec![
			"function_definition",
			"class_specifier",
			"struct_specifier",
			"enum_specifier",
			"namespace_definition",
		]
	}

	fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String> {
		let mut symbols = Vec::new();

		match node.kind() {
			"function_definition" => {
				// Find function name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "function_declarator" {
						for decl_child in child.children(&mut child.walk()) {
							if decl_child.kind() == "identifier" {
								if let Ok(name) = decl_child.utf8_text(contents.as_bytes()) {
									symbols.push(name.to_string());
								}
								break;
							}
						}
						break;
					}
				}

				// Extract variables from function body
				for child in node.children(&mut node.walk()) {
					if child.kind() == "compound_statement" {
						self.extract_cpp_variables(child, contents, &mut symbols);
						break;
					}
				}
			},
			"class_specifier" | "struct_specifier" | "enum_specifier" => {
				// Find class/struct/enum name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "name" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
					} else if child.kind() == "type_identifier" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
					}
				}

				// Extract member names
				self.extract_cpp_members(node, contents, &mut symbols);
			},
			"namespace_definition" => {
				// Find namespace name
				for child in node.children(&mut node.walk()) {
					if child.kind() == "identifier" || child.kind() == "namespace_identifier" {
						if let Ok(name) = child.utf8_text(contents.as_bytes()) {
							symbols.push(name.to_string());
						}
						break;
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
		// Check if this is a valid identifier
		if kind == "identifier" || kind == "type_identifier" || kind == "field_identifier" {
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

impl Cpp {
	/// Extract variable declarations in C++ compound statements
	fn extract_cpp_variables(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				match child.kind() {
					"declaration" => {
						// Handle variable declarations
						for decl_child in child.children(&mut child.walk()) {
							if decl_child.kind() == "init_declarator" || decl_child.kind() == "declarator" {
								for init_child in decl_child.children(&mut decl_child.walk()) {
									if init_child.kind() == "identifier" {
										if let Ok(name) = init_child.utf8_text(contents.as_bytes()) {
											if !symbols.contains(&name.to_string()) {
												symbols.push(name.to_string());
											}
										}
										break;
									}
								}
							}
						}
					},
					"compound_statement" => {
						// Recursively process nested blocks
						self.extract_cpp_variables(child, contents, symbols);
					},
					"if_statement" | "for_statement" | "while_statement" | "do_statement" => {
						// Process compound statements in control structures
						for stmt_child in child.children(&mut child.walk()) {
							if stmt_child.kind() == "compound_statement" {
								self.extract_cpp_variables(stmt_child, contents, symbols);
							}
						}
					},
					_ => {}
				}

				if !cursor.goto_next_sibling() { break; }
			}
		}
	}

	/// Extract members from class/struct/enum
	fn extract_cpp_members(&self, node: Node, contents: &str, symbols: &mut Vec<String>) {
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				let child = cursor.node();

				match child.kind() {
					"field_declaration" => {
						// Extract field names
						for field_child in child.children(&mut child.walk()) {
							if field_child.kind() == "field_identifier" || field_child.kind() == "identifier" {
								if let Ok(name) = field_child.utf8_text(contents.as_bytes()) {
									if !symbols.contains(&name.to_string()) {
										symbols.push(name.to_string());
									}
								}
							}
						}
					},
					"function_definition" => {
						// Handle method definitions
						for fn_child in child.children(&mut child.walk()) {
							if fn_child.kind() == "function_declarator" {
								for decl_child in fn_child.children(&mut fn_child.walk()) {
									if decl_child.kind() == "identifier" {
										if let Ok(name) = decl_child.utf8_text(contents.as_bytes()) {
											if !symbols.contains(&name.to_string()) {
												symbols.push(name.to_string());
											}
										}
										break;
									}
								}
								break;
							}
						}
					},
					"enum_specifier" => {
						// Extract enum constant names
						for enum_child in child.children(&mut child.walk()) {
							if enum_child.kind() == "enumerator_list" {
								for enum_list_child in enum_child.children(&mut enum_child.walk()) {
									if enum_list_child.kind() == "enumerator" {
										for enumerator_child in enum_list_child.children(&mut enum_list_child.walk()) {
											if enumerator_child.kind() == "identifier" {
												if let Ok(name) = enumerator_child.utf8_text(contents.as_bytes()) {
													if !symbols.contains(&name.to_string()) {
														symbols.push(name.to_string());
													}
												}
												break;
											}
										}
									}
								}
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
