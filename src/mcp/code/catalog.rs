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

/// How many tools `search_tools` prints: Anthropic's tool search tool returns 3 to 5
/// of the best matches, enough to pick from without a second search.
const SEARCH_RESULTS: usize = 5;

/// The standard BM25 parameters (Robertson and Zaragoza, 2009): how fast
/// repeats of a word stop adding to a score, and how much a long description
/// counts against it.
const BM25_SATURATION: f64 = 1.2;
const BM25_LENGTH_WEIGHT: f64 = 0.75;

/// Reciprocal rank fusion constant (Cormack, Clarke and Buettcher, 2009): a
/// tool's fused score is the sum of `1 / (RRF_K + rank)` over the rankings.
const RRF_K: f64 = 60.0;

/// Lucene's English stop words (`EnglishAnalyzer.ENGLISH_STOP_WORDS_SET`).
/// Rank fusion counts a tool's place in the word ranking, not its score, so
/// one shared "a" would rank a tool as high as a shared name does.
const STOP_WORDS: &[&str] = &[
	"a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it", "no", "not", "of",
	"on", "or", "such", "that", "the", "their", "then", "there", "these", "they", "this", "to", "was", "will", "with",
];

/// The words of a text for search: lowercase runs of letters and digits,
/// `find_all` and `findAll` alike split into their words, stop words left out.
fn search_terms(text: &str) -> Vec<String> {
	let mut terms = Vec::new();
	let mut current = String::new();
	let mut previous_lowercase = false;
	for character in text.chars() {
		let boundary = !character.is_alphanumeric() || (character.is_uppercase() && previous_lowercase);
		if boundary && !current.is_empty() {
			terms.push(std::mem::take(&mut current));
		}
		if character.is_alphanumeric() {
			current.extend(character.to_lowercase());
		}
		previous_lowercase = character.is_lowercase();
	}
	if !current.is_empty() {
		terms.push(current);
	}
	terms.retain(|term| !STOP_WORDS.contains(&term.as_str()));
	terms
}

/// The text of a tool that `search_tools` matches against: its server, name,
/// description and argument names.
fn search_text(server: &Server, tool: &Tool) -> String {
	let arguments = tool
		.schema
		.get("properties")
		.and_then(Value::as_object)
		.map(|properties| properties.keys().cloned().collect::<Vec<_>>().join(" "))
		.unwrap_or_default();
	format!("{} {}: {} {}", server.name, tool.name, tool.description, arguments)
}

/// Positions `0..scores.len()` ordered from the highest score down, ties in
/// list order.
fn by_score(scores: &[f64]) -> Vec<usize> {
	let mut order: Vec<usize> = (0..scores.len()).collect();
	order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
	order
}

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
	/// The MCP `outputSchema` it declares: the shape of the value its call
	/// resolves to. Without one a call resolves to the tool's text.
	pub output: Option<Value>,
}

impl Tool {
	/// What a call of the tool resolves to.
	pub fn result_type(&self) -> String {
		self.output
			.as_ref()
			.map_or_else(|| "string".to_string(), schema::result_type)
	}
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
	/// Add one tool. A tool whose script name another tool of the server took
	/// first is numbered (`get_item_2`), so every tool stays callable.
	pub fn add(&mut self, server: &str, tool: &str, description: &str, schema: Value) {
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
		let server = &mut self.servers[position];
		let base = js_ident(tool);
		let mut ident = base.clone();
		let mut number = 1;
		while server.tool(&ident).is_some() {
			number += 1;
			ident = format!("{base}_{number}");
		}
		server.tools.push(Tool {
			name: tool.to_string(),
			ident,
			description: description.to_string(),
			schema,
			output: crate::mcp::tool_shape::tool_output_schema(tool),
		});
	}

	pub fn is_empty(&self) -> bool {
		self.servers.is_empty()
	}

	pub fn server(&self, ident: &str) -> Option<&Server> {
		self.servers.iter().find(|server| server.ident == ident)
	}

	/// What the type checker knows a script may use: the sandbox's functions,
	/// and every tool as a method that takes its arguments and resolves to its
	/// result.
	pub fn declarations(&self) -> String {
		let mut text = String::from(SANDBOX_DECLARATIONS);
		for server in &self.servers {
			text.push_str(&format!("interface {} {{\n", server.type_name()));
			for tool in &server.tools {
				text.push_str(&format!(
					"\t{}({}): Promise<{}>;\n",
					tool.ident,
					schema::parameter(&tool.schema),
					tool.result_type()
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

	/// One line per server with how many tools it has: what the list of tools
	/// becomes when the full one is too long to send with every request.
	pub fn server_index(&self) -> String {
		self.servers
			.iter()
			.map(|server| match server.tools.len() {
				1 => format!("{} (1 tool)", server.ident),
				count => format!("{} ({count} tools)", server.ident),
			})
			.collect::<Vec<_>>()
			.join("\n")
	}

	/// Every tool of the catalog, in list order.
	fn tools(&self) -> impl Iterator<Item = (&Server, &Tool)> {
		self.servers
			.iter()
			.flat_map(|server| server.tools.iter().map(move |tool| (server, tool)))
	}

	/// The text `search_tools` embeds for each tool, in list order, cut to what
	/// the embedding model reads.
	pub fn search_texts(&self) -> Vec<String> {
		self.tools()
			.map(|(server, tool)| {
				// Never empty: the text holds the server and tool names.
				crate::embeddings::chunk_to_token_limit(
					&search_text(server, tool),
					crate::embeddings::EMBED_MAX_INPUT_TOKENS,
				)
				.swap_remove(0)
			})
			.collect()
	}

	/// What `search_tools(words)` prints: the tools that match the words best,
	/// as the list of tools shows them. Two rankings are fused: BM25 over each
	/// tool's words, which finds the exact names a model types, and cosine
	/// similarity of embeddings, which finds a tool described in other words
	/// ("remove a file" for `delete_path`). Anthropic's tool search offers the
	/// first, embedding search the second; together they miss less than either.
	pub async fn search(&self, words: &str) -> anyhow::Result<String> {
		let Some(query) = crate::embeddings::chunk_to_token_limit(words, crate::embeddings::EMBED_MAX_INPUT_TOKENS)
			.into_iter()
			.next()
		else {
			anyhow::bail!("search_tools takes the words to look for: search_tools(\"edit a file\")");
		};
		let texts = self.search_texts();
		let query_vector = crate::embeddings::embed(&query).await?;
		let tool_vectors = crate::embeddings::embed_many(&texts).await?;
		let similarity: Vec<f64> = tool_vectors
			.iter()
			.map(|vector| f64::from(crate::embeddings::cosine(&query_vector, vector)))
			.collect();
		Ok(self.ranked(words, &similarity))
	}

	/// [`Self::search`] once each tool's similarity to the words is known, in
	/// list order.
	pub fn ranked(&self, words: &str, similarity: &[f64]) -> String {
		let tools: Vec<(&Server, &Tool)> = self.tools().collect();
		let query = search_terms(words);
		let documents: Vec<Vec<String>> = tools
			.iter()
			.map(|(server, tool)| search_terms(&search_text(server, tool)))
			.collect();
		let count = documents.len() as f64;
		let average_length = documents.iter().map(Vec::len).sum::<usize>() as f64 / count.max(1.0);
		let keyword: Vec<f64> = documents
			.iter()
			.map(|terms| {
				query
					.iter()
					.map(|term| {
						let frequency = terms.iter().filter(|known| *known == term).count() as f64;
						if frequency == 0.0 {
							return 0.0;
						}
						let holding = documents.iter().filter(|other| other.contains(term)).count() as f64;
						let rarity = ((count - holding + 0.5) / (holding + 0.5) + 1.0).ln();
						let length =
							1.0 - BM25_LENGTH_WEIGHT + BM25_LENGTH_WEIGHT * terms.len() as f64 / average_length;
						rarity * frequency * (BM25_SATURATION + 1.0) / (frequency + BM25_SATURATION * length)
					})
					.sum::<f64>()
			})
			.collect();

		let mut fused = vec![0.0; tools.len()];
		// A tool no query word appears in has no keyword rank at all.
		let keyword_ranking = by_score(&keyword)
			.into_iter()
			.filter(|position| keyword[*position] > 0.0);
		for (rank, position) in keyword_ranking.enumerate() {
			fused[position] += 1.0 / (RRF_K + rank as f64 + 1.0);
		}
		for (rank, position) in by_score(similarity).into_iter().enumerate() {
			fused[position] += 1.0 / (RRF_K + rank as f64 + 1.0);
		}
		by_score(&fused)
			.into_iter()
			.take(SEARCH_RESULTS)
			.map(|position| listing(tools[position].0, tools[position].1))
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
		let mut text = call_form(server, tool);
		if !tool.description.trim().is_empty() {
			text.push_str("\n\n");
			text.push_str(tool.description.trim());
		}
		let arguments = schema::describe_arguments(&tool.schema);
		if !arguments.is_empty() {
			text.push_str("\n\nArguments:\n");
			text.push_str(&arguments);
		}
		if tool.output.is_some() {
			text.push_str(&format!("\n\nResolves to: {}", tool.result_type()));
		}
		text
	}
}

/// `server.tool({...})` with the fields of nested objects spelled out. Every place
/// that shows a call shows this one form. The list of tools once printed a nested
/// object as `object`, and models then made its fields up: `{op, target}` for the
/// `{operation, start}` of an edit operation.
pub fn call_form(server: &Server, tool: &Tool) -> String {
	format!(
		"{}.{}({})",
		server.ident,
		tool.ident,
		schema::signature(&tool.schema, schema::FULL_DEPTH)
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
	let mut text = call_form(server, tool);
	if tool.output.is_some() {
		text.push_str(&format!(" -> {}", tool.result_type()));
	}
	let summary = summary(&tool.description);
	if !summary.is_empty() {
		text.push_str(&format!(" // {summary}"));
	}
	text
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
