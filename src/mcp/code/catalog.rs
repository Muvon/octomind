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

//! The tools a script can call: MCP servers and their tools, under the
//! JavaScript names a script reaches them by.

use super::schema;
use serde_json::Value;

/// Names a server cannot take as a script global: the sandbox's own functions,
/// the standard globals a script expects to keep, and JavaScript keywords.
/// The sandbox's hidden helpers all start with [`HIDDEN_PREFIX`].
const RESERVED_GLOBALS: &[&str] = &[
	"print",
	"help",
	"input",
	"sleep",
	"console",
	"setTimeout",
	"globalThis",
	"Object",
	"Array",
	"String",
	"Number",
	"Boolean",
	"TypeError",
	"RangeError",
	"SyntaxError",
	"ReferenceError",
	"Symbol",
	"BigInt",
	"Math",
	"JSON",
	"Date",
	"RegExp",
	"Error",
	"Promise",
	"Map",
	"Set",
	"WeakMap",
	"WeakSet",
	"Proxy",
	"Reflect",
	"Function",
	"undefined",
	"NaN",
	"Infinity",
	"eval",
	"parseInt",
	"parseFloat",
	"isNaN",
	"isFinite",
	"async",
	"await",
	"break",
	"case",
	"catch",
	"class",
	"const",
	"continue",
	"debugger",
	"default",
	"delete",
	"do",
	"else",
	"enum",
	"export",
	"extends",
	"false",
	"finally",
	"for",
	"function",
	"if",
	"import",
	"in",
	"instanceof",
	"let",
	"new",
	"null",
	"return",
	"static",
	"super",
	"switch",
	"this",
	"throw",
	"true",
	"try",
	"typeof",
	"var",
	"void",
	"while",
	"with",
	"yield",
];

/// What the names of the sandbox's hidden helpers (`__call`, `__ok`) start with.
const HIDDEN_PREFIX: &str = "__";

/// TypeScript declarations of what the sandbox itself gives a script.
/// `__ok` and `__fail` receive the script's result; see `checker.js`.
const SANDBOX_DECLARATIONS: &str = "\
declare function print(...values: unknown[]): void;
declare const console: { log(...values: unknown[]): void; info(...values: unknown[]): void; warn(...values: unknown[]): void; error(...values: unknown[]): void; debug(...values: unknown[]): void };
declare function help(target?: string): void;
declare function sleep(milliseconds: number): Promise<void>;
declare function setTimeout(callback: (...args: any[]) => void, milliseconds?: number, ...args: any[]): number;
declare function __ok(value: unknown): void;
declare function __fail(error: unknown): void;
";

/// One tool of a server.
#[derive(Debug, Clone)]
pub struct Tool {
	/// Name the server knows the tool by.
	pub name: String,
	/// Name a script calls it by.
	pub ident: String,
	pub description: String,
	/// The tool's JSON input schema.
	pub schema: Value,
}

/// One MCP server and the tools a script may call on it.
#[derive(Debug, Clone)]
pub struct Server {
	/// Name the server is configured under.
	pub name: String,
	/// Global a script reaches it by.
	pub ident: String,
	pub tools: Vec<Tool>,
}

impl Server {
	pub fn tool(&self, ident: &str) -> Option<&Tool> {
		self.tools.iter().find(|tool| tool.ident == ident)
	}

	pub fn tool_idents(&self) -> Vec<&str> {
		self.tools.iter().map(|tool| tool.ident.as_str()).collect()
	}

	/// Name of the TypeScript interface that declares this server's tools.
	/// A named type keeps a type error short: it says `filesTools` where it
	/// would otherwise spell out every tool of the server.
	pub fn type_name(&self) -> String {
		format!("{}Tools", self.ident)
	}
}

/// Every server a script can call.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
	pub servers: Vec<Server>,
}

/// A name as a JavaScript identifier: every other character becomes `_`.
pub fn js_ident(name: &str) -> String {
	let mut ident: String = name
		.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
				c
			} else {
				'_'
			}
		})
		.collect();
	if ident.chars().next().is_none_or(|c| c.is_ascii_digit()) {
		ident.insert(0, '_');
	}
	ident
}

impl Catalog {
	/// Add one tool. Returns false when another tool of the server already
	/// took the same script name, which leaves the catalog unchanged.
	pub fn add(&mut self, server: &str, tool: &str, description: &str, schema: Value) -> bool {
		let position = match self.servers.iter().position(|known| known.name == server) {
			Some(position) => position,
			None => {
				let mut ident = js_ident(server);
				if ident.starts_with(HIDDEN_PREFIX) {
					ident.insert(0, 's');
				}
				while RESERVED_GLOBALS.contains(&ident.as_str()) || self.server(&ident).is_some() {
					ident.push('_');
				}
				self.servers.push(Server {
					name: server.to_string(),
					ident,
					tools: Vec::new(),
				});
				self.servers.len() - 1
			}
		};
		let ident = js_ident(tool);
		let server = &mut self.servers[position];
		if server.tool(&ident).is_some() {
			return false;
		}
		server.tools.push(Tool {
			name: tool.to_string(),
			ident,
			description: description.to_string(),
			schema,
		});
		true
	}

	pub fn is_empty(&self) -> bool {
		self.servers.is_empty()
	}

	pub fn server(&self, ident: &str) -> Option<&Server> {
		self.servers.iter().find(|server| server.ident == ident)
	}

	/// What the type checker knows a script may use: the sandbox's functions,
	/// `input` with the type of the value passed for it, and every tool as a
	/// method that takes its arguments and resolves to its text output.
	pub fn declarations(&self, input: Option<&Value>) -> String {
		let mut text = String::from(SANDBOX_DECLARATIONS);
		text.push_str(&format!(
			"declare const input: {};\n",
			input.map_or_else(|| "undefined".to_string(), schema::value_type)
		));
		for server in &self.servers {
			text.push_str(&format!("interface {} {{\n", server.type_name()));
			for tool in &server.tools {
				text.push_str(&format!(
					"\t{}({}): Promise<string>;\n",
					tool.ident,
					schema::parameter(&tool.schema)
				));
			}
			text.push_str(&format!(
				"}}\ndeclare const {}: {};\n",
				server.ident,
				server.type_name()
			));
		}
		text
	}

	/// One line per tool: `server.tool({arg: type, ...}) // what it does`.
	pub fn index(&self) -> String {
		self.servers
			.iter()
			.flat_map(|server| server.tools.iter().map(move |tool| listing(server, tool)))
			.collect::<Vec<_>>()
			.join("\n")
	}

	/// The tool a model means by `name`: the name its server knows it by
	/// (`view`), or the name the list shows (`files.view`).
	pub fn find(&self, name: &str) -> Option<(&Server, &Tool)> {
		self.servers.iter().find_map(|server| {
			server
				.tools
				.iter()
				.find(|tool| tool.name == name || format!("{}.{}", server.ident, tool.ident) == name)
				.map(|tool| (server, tool))
		})
	}

	/// What `help(target)` prints: every tool, the tools of one server with
	/// their summaries, or one tool in full.
	pub fn help(&self, target: &str) -> String {
		let target = target.trim();
		if target.is_empty() {
			return self.index();
		}
		let (server_ident, tool_ident) = match target.split_once('.') {
			Some((server, tool)) => (server, Some(tool)),
			None => (target, None),
		};
		let Some(server) = self.server(server_ident) else {
			let known: Vec<&str> = self.servers.iter().map(|server| server.ident.as_str()).collect();
			return format!("No server `{server_ident}`. Servers: {}", known.join(", "));
		};
		let Some(tool_ident) = tool_ident else {
			return server
				.tools
				.iter()
				.map(|tool| listing(server, tool))
				.collect::<Vec<_>>()
				.join("\n");
		};
		let Some(tool) = server.tool(tool_ident) else {
			return format!(
				"`{}.{}` is not a tool. Tools of {}: {}",
				server.ident,
				tool_ident,
				server.ident,
				server.tool_idents().join(", ")
			);
		};
		let mut text = call_form(server, tool, schema::FULL_DEPTH);
		if !tool.description.trim().is_empty() {
			text.push_str("\n\n");
			text.push_str(tool.description.trim());
		}
		let arguments = schema::describe_arguments(&tool.schema);
		if !arguments.is_empty() {
			text.push_str("\n\nArguments:\n");
			text.push_str(&arguments);
		}
		text
	}
}

/// `server.tool({...})` with nested objects spelled out `depth` levels deep.
pub fn call_form(server: &Server, tool: &Tool, depth: usize) -> String {
	format!(
		"{}.{}({})",
		server.ident,
		tool.ident,
		schema::signature(&tool.schema, depth)
	)
}

/// Longest summary of a tool in the list of tools. The list is sent with
/// every request, so it carries what a tool is for and its first rule of
/// use, not its manual: `help` prints that.
const SUMMARY_CHARS: usize = 240;

/// The start of a tool's description: its whole sentences that fit in
/// [`SUMMARY_CHARS`], or the first one cut short.
fn summary(description: &str) -> String {
	let text = description.split_whitespace().collect::<Vec<_>>().join(" ");
	if text.chars().count() <= SUMMARY_CHARS {
		return text;
	}
	let fitting: String = text.chars().take(SUMMARY_CHARS).collect();
	match fitting.rfind(". ") {
		Some(end) => fitting[..=end].to_string(),
		None => format!("{fitting}..."),
	}
}

/// A tool as the list of tools shows it: how to call it, then what it does.
fn listing(server: &Server, tool: &Tool) -> String {
	let summary = summary(&tool.description);
	if summary.is_empty() {
		call_form(server, tool, 0)
	} else {
		format!("{} // {}", call_form(server, tool, 0), summary)
	}
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
