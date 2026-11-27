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

use super::super::fixtures::catalog;
use super::*;
use serde_json::json;

#[test]
fn names_become_javascript_identifiers() {
	assert_eq!(js_ident("my-search"), "my_search");
	assert_eq!(js_ident("find.all"), "find_all");
	assert_eq!(js_ident("9lives"), "_9lives");
	assert_eq!(js_ident(""), "_");
}

#[test]
fn server_named_like_a_global_gets_a_suffix() {
	let mut catalog = Catalog::default();
	catalog.add("console", "log", "", json!({"type": "object"}));
	catalog.add("new", "thing", "", json!({"type": "object"}));
	assert_eq!(catalog.servers[0].ident, "console_");
	assert_eq!(catalog.servers[1].ident, "new_");
}

#[test]
fn two_servers_with_one_script_name_stay_distinct() {
	let mut catalog = Catalog::default();
	catalog.add("a-b", "x", "", json!({"type": "object"}));
	catalog.add("a.b", "x", "", json!({"type": "object"}));
	assert_eq!(catalog.servers[0].ident, "a_b");
	assert_eq!(catalog.servers[1].ident, "a_b_");
}

#[test]
fn second_tool_with_the_same_script_name_is_numbered() {
	let mut catalog = Catalog::default();
	catalog.add("s", "get-item", "", json!({"type": "object"}));
	catalog.add("s", "get.item", "", json!({"type": "object"}));
	catalog.add("s", "get_item", "", json!({"type": "object"}));
	let idents: Vec<&str> = catalog.servers[0]
		.tools
		.iter()
		.map(|tool| tool.ident.as_str())
		.collect();
	assert_eq!(idents, vec!["get_item", "get_item_2", "get_item_3"]);
}

/// A tool that declares an output schema resolves to that shape: the list
/// shows it, the declarations type the call with it and `help` names it.
#[test]
fn a_declared_output_schema_types_the_result() {
	crate::mcp::tool_shape::register_tool_output_schema(
		"catalog-test-stats",
		Some(json!({"type": "object", "properties": {"lines": {"type": "integer"}}, "required": ["lines"]})),
	);
	let mut catalog = Catalog::default();
	catalog.add("s", "catalog-test-stats", "Count lines.", json!({"type": "object"}));
	catalog.add("s", "catalog-test-text", "", json!({"type": "object"}));
	assert_eq!(
		catalog.index(),
		"s.catalog_test_stats() -> {lines: number} // Count lines.\ns.catalog_test_text()"
	);
	let declarations = catalog.declarations();
	assert!(declarations.contains("catalog_test_stats(args?: Record<string, any>): Promise<{lines: number}>;"));
	assert!(declarations.contains("catalog_test_text(args?: Record<string, any>): Promise<string>;"));
	assert!(catalog
		.help("s.catalog_test_stats")
		.ends_with("Resolves to: {lines: number}"));
	assert!(!catalog.help("s.catalog_test_text").contains("Resolves to"));
}

#[test]
fn index_lists_how_to_call_each_tool_and_what_it_does() {
	assert_eq!(
		catalog().index(),
		"files.read({path: string, limit?: number}) // Read a file. Returns its text.\n\
		 files.write({content: string, path: string}) // Write a file.\n\
		 my_search.find_all({query: string, mode?: \"code\"|\"docs\", page?: {size: number, after?: string|null}}) // Search the index."
	);
}

/// The list is all a model has when it writes a call, so it names the fields
/// of an object inside a list too.
#[test]
fn the_list_names_the_fields_of_nested_arguments() {
	let mut catalog = Catalog::default();
	catalog.add(
		"files",
		"edit",
		"Edit a file.",
		json!({
			"type": "object",
			"properties": {
				"operations": {
					"type": "array",
					"items": {
						"type": "object",
						"properties": {
							"operation": {"enum": ["insert", "replace"]},
							"start": {"type": "string"},
							"copy_from": {
								"type": "object",
								"properties": {"path": {"type": "string"}},
								"required": ["path"]
							}
						},
						"required": ["operation", "start"]
					}
				},
				"path": {"type": "string"}
			},
			"required": ["operations", "path"]
		}),
	);
	assert_eq!(
		catalog.index(),
		"files.edit({operations: ({operation: \"insert\"|\"replace\", start: string, copy_from?: {path: string}})[], path: string}) // Edit a file."
	);
}

#[test]
fn a_long_description_is_listed_by_its_first_sentences() {
	let sentence = "It reads the named file from the working directory and gives back the text it holds. ";
	let mut catalog = Catalog::default();
	catalog.add(
		"files",
		"read",
		&sentence.repeat(4),
		serde_json::json!({"type": "object"}),
	);
	catalog.add("files", "blob", &"x".repeat(400), serde_json::json!({"type": "object"}));
	assert_eq!(
		catalog.index(),
		format!(
			"files.read() // {}\nfiles.blob() // {}...",
			sentence.repeat(2).trim_end(),
			"x".repeat(240)
		)
	);
}

#[test]
fn a_tool_without_a_description_is_listed_by_its_call_alone() {
	let mut catalog = Catalog::default();
	catalog.add("files", "read", "  ", serde_json::json!({"type": "object"}));
	assert_eq!(catalog.index(), "files.read()");
}

#[test]
fn help_for_a_server_lists_its_tools() {
	assert_eq!(
		catalog().help("files"),
		"files.read({path: string, limit?: number}) // Read a file. Returns its text.\n\
		 files.write({content: string, path: string}) // Write a file."
	);
}

#[test]
fn help_for_a_tool_spells_out_nested_arguments() {
	let text = catalog().help("my_search.find_all");
	assert!(text.starts_with(
		"my_search.find_all({query: string, mode?: \"code\"|\"docs\", page?: {size: number, after?: string|null}})"
	));
	assert!(text.contains("Search the index."));
	assert!(text.contains("  page?: object\n    size: number - Hits per page"));
}

#[test]
fn help_names_what_exists_when_the_target_does_not() {
	assert_eq!(catalog().help("nope"), "No server `nope`. Servers: files, my_search");
	assert_eq!(
		catalog().help("files.nope"),
		"`files.nope` is not a tool. Tools of files: read, write"
	);
}

#[test]
fn declarations_type_every_tool() {
	let text = catalog().declarations();
	assert!(
		text.contains("interface filesTools {\n\tread(args: {path: string, limit?: number}): Promise<string>;\n"),
		"{text}"
	);
	assert!(text.contains("}\ndeclare const files: filesTools;\n"), "{text}");
	assert!(
		text.contains("\tfind_all(args: {query: string, mode?: \"code\"|\"docs\", page?: {size: number, after?: string|null}}): Promise<string>;\n"),
		"{text}"
	);
}

#[test]
fn help_without_a_target_is_the_index() {
	assert_eq!(catalog().help(""), catalog().index());
}

/// The embedding model finds a tool described in words its text does not
/// use, which word matching alone cannot.
#[tokio::test]
async fn search_finds_a_tool_by_meaning() {
	let mut catalog = Catalog::default();
	let object = || json!({"type": "object", "properties": {"path": {"type": "string"}}});
	catalog.add("files", "delete_path", "Unlink an entry from the disk.", object());
	catalog.add("files", "read_text", "Return the contents of a document.", object());
	catalog.add(
		"tracker",
		"list_issues",
		"Show the open tickets of a project.",
		object(),
	);
	let found = catalog.search("remove a file").await.expect("search runs");
	assert!(found.starts_with("files.delete_path("), "{found}");
	assert!(catalog.search("  ").await.is_err());
}
