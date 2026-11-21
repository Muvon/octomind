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

use super::*;
use serde_json::json;

fn read_schema() -> Value {
	json!({
		"type": "object",
		"properties": {
			"limit": { "type": "integer" },
			"path": { "type": "string" },
			"range": { "anyOf": [{ "type": "integer" }, { "type": "string" }] },
			"tags": { "type": "array", "items": { "type": "string" } }
		},
		"required": ["path"]
	})
}

#[test]
fn signature_puts_required_arguments_first() {
	assert_eq!(
		signature(&read_schema(), 0),
		"{path: string, limit?: number, range?: number|string, tags?: string[]}"
	);
}

#[test]
fn signature_is_empty_for_a_tool_without_arguments() {
	assert_eq!(signature(&json!({"type": "object"}), 0), "");
	assert_eq!(signature(&json!({"type": "object", "properties": {}}), 0), "");
}

#[test]
fn a_union_array_is_parenthesised() {
	let schema = json!({
		"type": "object",
		"properties": { "ids": { "type": "array", "items": { "type": ["string", "integer"] } } }
	});
	assert_eq!(signature(&schema, 0), "{ids?: (string|number)[]}");
}

#[test]
fn a_reference_cycle_ends() {
	let schema = json!({
		"type": "object",
		"properties": { "node": { "$ref": "#/$defs/Node" } },
		"$defs": { "Node": { "$ref": "#/$defs/Node" } }
	});
	assert_eq!(signature(&schema, 2), "{node?: any}");
}

/// The usual schema of "any JSON value": text, or a list of such values, or
/// a pair of them.
fn self_containing_schema() -> serde_json::Value {
	json!({
		"type": "object",
		"properties": { "value": { "$ref": "#/$defs/Json" } },
		"required": ["value"],
		"$defs": {
			"Json": {
				"anyOf": [
					{ "type": "string" },
					{ "type": "array", "items": { "$ref": "#/$defs/Json" } },
					{ "type": "array", "items": { "type": "array", "items": { "$ref": "#/$defs/Json" } } }
				]
			}
		}
	})
}

#[test]
fn a_schema_that_contains_itself_is_spelled_out_once() {
	assert_eq!(signature(&self_containing_schema(), 2), "{value: string|any[]|any[][]}");
	assert_eq!(
		parameter(&self_containing_schema()),
		"args: {value: string|any[]|any[][]}"
	);
	assert!(describe_arguments(&self_containing_schema()).contains("value: string|any[]|any[][]"));
}

#[test]
fn arguments_of_a_schema_that_contains_itself_are_checked() {
	let schema = self_containing_schema();
	assert!(validate(&schema, &json!({"value": ["a", ["b", ["c"]]]})).is_empty());
	assert_eq!(
		validate(&schema, &json!({"value": ["a", 1]})),
		vec!["`value`: expected string|any[]|any[][], got array"]
	);
}

#[test]
fn text_that_spells_another_type_is_read_as_that_type() {
	let schema = json!({
		"type": "object",
		"properties": {
			"path": { "type": "string" },
			"depth": { "type": ["integer", "null"] },
			"ratio": { "type": "number" },
			"hidden": { "anyOf": [{ "type": "boolean" }, { "type": "null" }] },
			"label": { "type": ["string", "null"] },
			"ops": { "type": "array", "items": { "type": "object", "properties": { "line": { "type": "integer" } } } },
			"anything": {}
		}
	});
	let mut arguments = json!({
		"path": "3",
		"depth": "3",
		"ratio": "0.5",
		"hidden": "true",
		"label": "null",
		"ops": [{ "line": "12" }, { "line": "x" }],
		"anything": "false",
		"undeclared": "7"
	});
	coerce(&schema, &mut arguments);
	assert_eq!(
		arguments,
		json!({
			"path": "3",
			"depth": 3,
			"ratio": 0.5,
			"hidden": true,
			"label": "null",
			"ops": [{ "line": 12 }, { "line": "x" }],
			"anything": "false",
			"undeclared": "7"
		})
	);

	let mut absent = json!({ "depth": "null", "hidden": "null" });
	coerce(&schema, &mut absent);
	assert_eq!(absent, json!({ "depth": null, "hidden": null }));

	// A list written as its JSON text, with numbers inside written as text.
	let mut listed = json!({ "ops": "[{\"line\": \"12\"}, {\"line\": 3}]", "path": "[1]" });
	coerce(&schema, &mut listed);
	assert_eq!(listed, json!({ "ops": [{ "line": 12 }, { "line": 3 }], "path": "[1]" }));
	// Text that is not a list stays as it is, for the check to report.
	let mut broken = json!({ "ops": "[{\"line\": 12" });
	coerce(&schema, &mut broken);
	assert_eq!(broken, json!({ "ops": "[{\"line\": 12" }));

	// A fraction is not an integer, whatever it is written as.
	let mut fraction = json!({ "depth": "1.5" });
	coerce(&schema, &mut fraction);
	assert_eq!(fraction, json!({ "depth": "1.5" }));
}

#[test]
fn an_enum_without_values_allows_nothing() {
	let schema = json!({
		"type": "object",
		"properties": { "mode": { "enum": [] }, "other": { "anyOf": [] } }
	});
	assert_eq!(signature(&schema, 0), "{mode?: never, other?: any}");
}

#[test]
fn valid_arguments_pass() {
	assert!(validate(
		&read_schema(),
		&json!({"path": "a", "limit": 3, "tags": ["x"], "range": "1-2"})
	)
	.is_empty());
	assert!(validate(&read_schema(), &json!({"path": "a", "limit": 3.0})).is_empty());
}

#[test]
fn a_missing_required_argument_is_named() {
	assert_eq!(
		validate(&read_schema(), &json!({"limit": 3})),
		vec!["missing required argument `path`"]
	);
}

#[test]
fn a_wrong_type_is_named_with_what_was_expected() {
	assert_eq!(
		validate(&read_schema(), &json!({"path": 5})),
		vec!["`path`: expected string, got number"]
	);
	assert_eq!(
		validate(&read_schema(), &json!({"path": "a", "limit": 1.5})),
		vec!["`limit`: expected number, got number"]
	);
	assert_eq!(
		validate(&read_schema(), &json!({"path": "a", "tags": ["x", 2]})),
		vec!["`tags[1]`: expected string, got number"]
	);
	assert_eq!(
		validate(&read_schema(), &json!({"path": "a", "range": true})),
		vec!["`range`: expected number|string, got boolean"]
	);
}

#[test]
fn an_undeclared_argument_is_a_problem_unless_the_schema_allows_extras() {
	assert_eq!(
		validate(&read_schema(), &json!({"path": "a", "file": "b"})),
		vec!["unknown argument `file` (known: limit, path, range, tags)"]
	);
	let mut open = read_schema();
	open["additionalProperties"] = json!(true);
	assert!(validate(&open, &json!({"path": "a", "file": "b"})).is_empty());
	let mut typed = read_schema();
	typed["additionalProperties"] = json!({"type": "string"});
	assert!(validate(&typed, &json!({"path": "a", "file": "b"})).is_empty());
	assert_eq!(
		validate(&typed, &json!({"path": "a", "file": 1})),
		vec!["`file`: expected string, got number"]
	);
}

#[test]
fn a_schema_without_properties_accepts_any_object() {
	assert!(validate(&json!({"type": "object"}), &json!({"anything": 1})).is_empty());
}

#[test]
fn arguments_must_be_an_object() {
	assert_eq!(
		validate(&read_schema(), &json!("a")),
		vec!["arguments: expected object, got string"]
	);
}

#[test]
fn enum_and_nested_reference_are_checked() {
	let schema = json!({
		"type": "object",
		"properties": {
			"mode": { "enum": ["code", "docs"] },
			"page": { "$ref": "#/$defs/Page" }
		},
		"$defs": { "Page": { "type": "object", "properties": { "size": { "type": "integer" } }, "required": ["size"] } }
	});
	assert_eq!(
		validate(&schema, &json!({"mode": "web"})),
		vec!["`mode`: expected one of \"code\"|\"docs\", got \"web\""]
	);
	assert_eq!(
		validate(&schema, &json!({"page": {}})),
		vec!["missing required argument `page.size`"]
	);
	assert_eq!(
		validate(&schema, &json!({"page": {"size": "ten"}})),
		vec!["`page.size`: expected number, got string"]
	);
}

#[test]
fn names_that_are_not_identifiers_are_quoted() {
	let schema = json!({
		"type": "object",
		"properties": { "$top": { "type": "integer" }, "max-results": { "type": "integer" } },
		"additionalProperties": true
	});
	assert_eq!(
		signature(&schema, 0),
		"{$top?: number, \"max-results\"?: number, [extra: string]: any}"
	);
}

#[test]
fn enum_values_that_are_not_primitives_have_no_literal_type() {
	let schema = json!({ "type": "object", "properties": { "shape": { "enum": ["a", 1, null, {"x": 1}] } } });
	assert_eq!(signature(&schema, 0), "{shape?: \"a\"|1|null|any}");
}

#[test]
fn a_declaration_parameter_is_optional_when_nothing_is_required() {
	assert_eq!(
		parameter(&read_schema()),
		"args: {path: string, limit?: number, range?: number|string, tags?: string[]}"
	);
	let optional = json!({ "type": "object", "properties": { "limit": { "type": "integer" } } });
	assert_eq!(parameter(&optional), "args?: {limit?: number}");
	assert_eq!(parameter(&json!({"type": "object"})), "args?: Record<string, any>");
}

#[test]
fn a_value_is_typed_as_exactly_as_it_is() {
	let value = json!({
		"count": 3,
		"flag": true,
		"long": "x".repeat(200),
		"modes": ["code", "docs", "code"],
		"multi-line": "a\nb",
		"none": null,
		"nothing": []
	});
	assert_eq!(
		value_type(&value),
		"{count: 3, flag: true, long: string, modes: (\"code\"|\"docs\")[], \"multi-line\": string, none: null, nothing: never[]}"
	);
}

#[test]
fn argument_descriptions_are_listed_with_nesting() {
	let schema = json!({
		"type": "object",
		"properties": {
			"items": {
				"type": "array",
				"description": "Rows to add",
				"items": { "type": "object", "properties": { "id": { "type": "string", "description": "Row id" } }, "required": ["id"] }
			}
		},
		"required": ["items"]
	});
	assert_eq!(
		describe_arguments(&schema),
		"  items: object[] - Rows to add\n    id: string - Row id"
	);
}
