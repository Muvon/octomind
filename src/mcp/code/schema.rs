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

//! A tool's JSON input schema, read two ways: rendered as a TypeScript type
//! (the call signature the model reads and the type checker enforces), and
//! used to check the arguments of a call right before it is sent.

use serde_json::{Map, Value};

/// Nested object levels `help` spells out for one tool.
pub const FULL_DEPTH: usize = 3;

/// `$ref` hops followed before a schema node is taken as it is, so a
/// reference cycle cannot loop.
const MAX_REF_HOPS: usize = 8;

/// How many schema nodes deep a call's arguments are checked. A schema may
/// contain itself (a JSON value is an array of JSON values); a value nested
/// deeper than this is accepted as it is.
const MAX_NESTING: usize = 24;

/// Follow local `$ref` pointers (`#/$defs/Name`) to the node they name.
fn resolve<'a>(root: &'a Value, mut node: &'a Value) -> &'a Value {
	for _ in 0..MAX_REF_HOPS {
		let Some(pointer) = node
			.get("$ref")
			.and_then(Value::as_str)
			.and_then(|reference| reference.strip_prefix('#'))
		else {
			return node;
		};
		match root.pointer(pointer) {
			Some(target) => node = target,
			None => return node,
		}
	}
	node
}

fn properties(node: &Value) -> Option<&Map<String, Value>> {
	node.get("properties")
		.and_then(Value::as_object)
		.filter(|properties| !properties.is_empty())
}

fn required(node: &Value) -> Vec<&str> {
	node.get("required")
		.and_then(Value::as_array)
		.map(|names| names.iter().filter_map(Value::as_str).collect())
		.unwrap_or_default()
}

/// Property names, required ones first.
fn ordered<'a>(node: &'a Value, properties: &'a Map<String, Value>) -> Vec<(&'a str, &'a Value, bool)> {
	let required = required(node);
	let mut fields: Vec<(&str, &Value, bool)> = properties
		.iter()
		.map(|(name, schema)| (name.as_str(), schema, required.contains(&name.as_str())))
		.collect();
	fields.sort_by_key(|(_, _, required)| !required);
	fields
}

/// A property name as TypeScript writes it: bare when it is an identifier,
/// quoted otherwise.
fn property_name(name: &str) -> String {
	let mut characters = name.chars();
	let identifier = characters
		.next()
		.is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
		&& characters.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
	if identifier {
		name.to_string()
	} else {
		Value::String(name.to_string()).to_string()
	}
}

/// A schema `enum` or `const` value as a literal type. Only primitives have one.
fn literal(value: &Value) -> String {
	match value {
		Value::Array(_) | Value::Object(_) => "any".to_string(),
		primitive => primitive.to_string(),
	}
}

fn union(mut types: Vec<String>) -> String {
	let mut seen = Vec::new();
	types.retain(|name| {
		let fresh = !seen.contains(name);
		if fresh {
			seen.push(name.clone());
		}
		fresh
	});
	types.join("|")
}

/// The TypeScript-style type of a schema node. `depth` is how many nested
/// object levels are spelled out; below it an object is just `object`.
///
/// `open` holds the referenced nodes being spelled out right now. A schema
/// may contain itself (a JSON value is an array of JSON values): meeting one
/// of them again ends that branch with `any` instead of never ending.
fn type_of<'a>(root: &'a Value, node: &'a Value, depth: usize, open: &mut Vec<&'a Value>) -> String {
	let target = resolve(root, node);
	if std::ptr::eq(target, node) {
		return shape(root, node, depth, open);
	}
	if open.iter().any(|entered| std::ptr::eq(*entered, target)) {
		return "any".to_string();
	}
	open.push(target);
	let name = shape(root, target, depth, open);
	open.pop();
	name
}

/// The type of a node that is not a reference.
fn shape<'a>(root: &'a Value, node: &'a Value, depth: usize, open: &mut Vec<&'a Value>) -> String {
	if let Some(values) = node.get("enum").and_then(Value::as_array) {
		// No value is allowed, which TypeScript spells `never`.
		if values.is_empty() {
			return "never".to_string();
		}
		return union(values.iter().map(literal).collect());
	}
	if let Some(value) = node.get("const") {
		return literal(value);
	}
	for key in ["anyOf", "oneOf"] {
		if let Some(branches) = node.get(key).and_then(Value::as_array).filter(|b| !b.is_empty()) {
			let mut types = Vec::with_capacity(branches.len());
			for branch in branches {
				types.push(type_of(root, branch, depth, open));
			}
			return union(types);
		}
	}
	match node.get("type") {
		Some(Value::String(name)) => named_type(root, node, name, depth, open),
		Some(Value::Array(names)) => {
			let mut types = Vec::with_capacity(names.len());
			for name in names.iter().filter_map(Value::as_str) {
				types.push(named_type(root, node, name, depth, open));
			}
			union(types)
		}
		_ if properties(node).is_some() => named_type(root, node, "object", depth, open),
		_ => "any".to_string(),
	}
}

fn named_type<'a>(root: &'a Value, node: &'a Value, name: &str, depth: usize, open: &mut Vec<&'a Value>) -> String {
	match name {
		"integer" | "number" => "number".to_string(),
		"array" => {
			let item = match node.get("items") {
				Some(items) => type_of(root, items, depth, open),
				None => "any".to_string(),
			};
			if item.contains('|') {
				format!("({item})[]")
			} else {
				format!("{item}[]")
			}
		}
		"object" => match properties(node) {
			Some(properties) if depth > 0 => format!("{{{}}}", fields(root, node, properties, depth - 1, open)),
			Some(_) => "object".to_string(),
			None => "Record<string, any>".to_string(),
		},
		other => other.to_string(),
	}
}

/// Whether an object schema accepts arguments it does not declare.
fn accepts_extras(node: &Value) -> bool {
	matches!(
		node.get("additionalProperties"),
		Some(Value::Bool(true) | Value::Object(_))
	)
}

fn fields<'a>(
	root: &'a Value,
	node: &'a Value,
	properties: &'a Map<String, Value>,
	depth: usize,
	open: &mut Vec<&'a Value>,
) -> String {
	let mut members = Vec::with_capacity(properties.len());
	for (name, schema, required) in ordered(node, properties) {
		let optional = if required { "" } else { "?" };
		members.push(format!(
			"{}{optional}: {}",
			property_name(name),
			type_of(root, schema, depth, open)
		));
	}
	if accepts_extras(node) {
		members.push("[extra: string]: any".to_string());
	}
	members.join(", ")
}

/// The argument object of a tool as a signature: `{path: string, limit?: number}`.
/// Empty when the tool declares no arguments.
pub fn signature(schema: &Value, depth: usize) -> String {
	let node = resolve(schema, schema);
	match properties(node) {
		Some(properties) => format!("{{{}}}", fields(schema, node, properties, depth, &mut Vec::new())),
		None => String::new(),
	}
}

/// The parameter list of a tool's TypeScript declaration: `args: {path: string}`,
/// optional when no argument is required.
pub fn parameter(schema: &Value) -> String {
	let node = resolve(schema, schema);
	match properties(node) {
		Some(properties) => {
			let optional = if required(node).is_empty() { "?" } else { "" };
			format!(
				"args{optional}: {{{}}}",
				fields(schema, node, properties, FULL_DEPTH, &mut Vec::new())
			)
		}
		None => "args?: Record<string, any>".to_string(),
	}
}

/// The type of what a tool with this output schema resolves to.
pub fn result_type(schema: &Value) -> String {
	type_of(schema, schema, FULL_DEPTH, &mut Vec::new())
}

/// One line per argument with its type and description, nested arguments
/// indented under their parent.
pub fn describe_arguments(schema: &Value) -> String {
	let mut lines = Vec::new();
	argument_lines(schema, schema, 1, FULL_DEPTH, &mut lines);
	lines.join("\n")
}

fn argument_lines(root: &Value, node: &Value, indent: usize, depth: usize, lines: &mut Vec<String>) {
	let node = resolve(root, node);
	let Some(properties) = properties(node) else {
		return;
	};
	for (name, schema, required) in ordered(node, properties) {
		let resolved = resolve(root, schema);
		let description = [schema, resolved]
			.iter()
			.find_map(|node| node.get("description").and_then(Value::as_str))
			.map(|text| format!(" - {}", text.trim().replace('\n', " ")))
			.unwrap_or_default();
		lines.push(format!(
			"{}{}{}: {}{}",
			"  ".repeat(indent),
			name,
			if required { "" } else { "?" },
			type_of(root, schema, 0, &mut Vec::new()),
			description
		));
		if depth > 0 {
			let nested = resolved.get("items").map_or(resolved, |items| resolve(root, items));
			argument_lines(root, nested, indent + 1, depth - 1, lines);
		}
	}
}

fn kind(value: &Value) -> &'static str {
	match value {
		Value::Null => "null",
		Value::Bool(_) => "boolean",
		Value::Number(_) => "number",
		Value::String(_) => "string",
		Value::Array(_) => "array",
		Value::Object(_) => "object",
	}
}

fn has_type(value: &Value, name: &str) -> bool {
	match name {
		"string" => value.is_string(),
		"number" => value.is_number(),
		"integer" => value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|number| number.fract() == 0.0),
		"boolean" => value.is_boolean(),
		"null" => value.is_null(),
		"array" => value.is_array(),
		"object" => value.is_object(),
		_ => true,
	}
}

fn argument_path(path: &str) -> String {
	if path.is_empty() {
		"arguments".to_string()
	} else {
		format!("`{path}`")
	}
}

fn child(path: &str, key: &str) -> String {
	if path.is_empty() {
		key.to_string()
	} else {
		format!("{path}.{key}")
	}
}

/// The type names a schema node allows, its `anyOf`/`oneOf` branches included.
fn allowed_types<'a>(root: &'a Value, node: &'a Value) -> Vec<&'a str> {
	let mut names = Vec::new();
	let mut add = |node: &'a Value| match node.get("type") {
		Some(Value::String(name)) => names.push(name.as_str()),
		Some(Value::Array(listed)) => names.extend(listed.iter().filter_map(Value::as_str)),
		_ => {}
	};
	add(node);
	for key in ["anyOf", "oneOf"] {
		for branch in node.get(key).and_then(Value::as_array).into_iter().flatten() {
			add(resolve(root, branch));
		}
	}
	names
}

/// Read text as the number, boolean, null, list or object it spells, where
/// the schema allows that type and does not allow text.
///
/// For the arguments of a call a model made without having been sent the
/// tool's schema: with nothing to type them by, models write `"3"`,
/// `"true"` and a list as its JSON text. Text stays text wherever the schema
/// takes text.
pub fn coerce(schema: &Value, arguments: &mut Value) {
	retype(schema, schema, arguments, MAX_NESTING);
}

fn retype(root: &Value, declared: &Value, value: &mut Value, nesting: usize) {
	let Some(nesting) = nesting.checked_sub(1) else {
		return;
	};
	let node = resolve(root, declared);
	match value {
		Value::String(text) => {
			let types = allowed_types(root, node);
			if types.is_empty() || types.contains(&"string") {
				return;
			}
			let allows = |name: &str| types.contains(&name);
			let text = text.trim();
			let spelled = match text {
				"true" if allows("boolean") => Some(Value::Bool(true)),
				"false" if allows("boolean") => Some(Value::Bool(false)),
				"null" if allows("null") => Some(Value::Null),
				_ => None,
			}
			.or_else(|| {
				if !allows("integer") && !allows("number") {
					return None;
				}
				text.parse::<i64>().map(Value::from).ok().or_else(|| {
					let number = text.parse::<f64>().ok().filter(|_| allows("number"))?;
					serde_json::Number::from_f64(number).map(Value::Number)
				})
			})
			.or_else(|| {
				serde_json::from_str::<Value>(text)
					.ok()
					.filter(|parsed| (parsed.is_array() && allows("array")) || (parsed.is_object() && allows("object")))
			});
			if let Some(spelled) = spelled {
				*value = spelled;
				// What a list or an object holds was written the same way.
				retype(root, declared, value, nesting);
			}
		}
		Value::Object(members) => {
			let Some(properties) = properties(node) else {
				return;
			};
			for (name, member) in members {
				if let Some(schema) = properties.get(name) {
					retype(root, schema, member, nesting);
				}
			}
		}
		Value::Array(items) => {
			let Some(schema) = node.get("items") else {
				return;
			};
			for item in items {
				retype(root, schema, item, nesting);
			}
		}
		_ => {}
	}
}

/// Check the arguments of a call against the tool's schema, right before the
/// call is sent. Returns one message per problem; empty when the call is valid.
///
/// The type check has already passed by then, but it cannot see what the
/// types do not say: a value parsed from JSON is `any`, and a schema's
/// `integer` is just `number` to TypeScript.
///
/// An argument the schema does not declare is a problem unless the schema
/// allows extra ones: a misspelled optional argument would otherwise be
/// dropped by the server without a word.
pub fn validate(schema: &Value, arguments: &Value) -> Vec<String> {
	let mut problems = Vec::new();
	check(schema, schema, arguments, "", MAX_NESTING, &mut problems);
	problems
}

fn check(root: &Value, declared: &Value, value: &Value, path: &str, nesting: usize, problems: &mut Vec<String>) {
	let Some(nesting) = nesting.checked_sub(1) else {
		return;
	};
	let node = resolve(root, declared);
	for key in ["anyOf", "oneOf"] {
		let Some(branches) = node.get(key).and_then(Value::as_array).filter(|b| !b.is_empty()) else {
			continue;
		};
		let fits = branches.iter().any(|branch| {
			let mut branch_problems = Vec::new();
			check(root, branch, value, path, nesting, &mut branch_problems);
			branch_problems.is_empty()
		});
		if !fits {
			problems.push(format!(
				"{}: expected {}, got {}",
				argument_path(path),
				type_of(root, declared, 1, &mut Vec::new()),
				kind(value)
			));
			return;
		}
	}
	if let Some(allowed) = node.get("enum").and_then(Value::as_array) {
		if !allowed.contains(value) {
			problems.push(format!(
				"{}: expected one of {}, got {}",
				argument_path(path),
				type_of(root, declared, 0, &mut Vec::new()),
				value
			));
			return;
		}
	}
	if let Some(constant) = node.get("const") {
		if constant != value {
			problems.push(format!("{}: expected {}, got {}", argument_path(path), constant, value));
			return;
		}
	}
	let types: Vec<&str> = match node.get("type") {
		Some(Value::String(name)) => vec![name.as_str()],
		Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
		_ => Vec::new(),
	};
	if !types.is_empty() && !types.iter().any(|name| has_type(value, name)) {
		problems.push(format!(
			"{}: expected {}, got {}",
			argument_path(path),
			type_of(root, declared, 0, &mut Vec::new()),
			kind(value)
		));
		return;
	}
	match value {
		Value::Object(given) => {
			let declared = properties(node);
			for name in required(node) {
				if !given.contains_key(name) {
					problems.push(format!("missing required argument `{}`", child(path, name)));
				}
			}
			for (key, item) in given {
				let item_path = child(path, key);
				match declared.and_then(|declared| declared.get(key)) {
					Some(schema) => check(root, schema, item, &item_path, nesting, problems),
					None if accepts_extras(node) => {
						if let Some(extra @ Value::Object(_)) = node.get("additionalProperties") {
							check(root, extra, item, &item_path, nesting, problems);
						}
					}
					None => {
						if let Some(declared) = declared {
							let known: Vec<&str> = declared.keys().map(String::as_str).collect();
							problems.push(format!(
								"unknown argument `{}` (known: {})",
								item_path,
								known.join(", ")
							));
						}
					}
				}
			}
		}
		Value::Array(items) => {
			if let Some(schema) = node.get("items") {
				for (index, item) in items.iter().enumerate() {
					check(root, schema, item, &format!("{path}[{index}]"), nesting, problems);
				}
			}
		}
		_ => {}
	}
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
