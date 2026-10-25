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

//! Terminal prompts for MCP elicitation (`elicitation/create`, form and URL modes).
//!
//! The interactive session keeps tty ECHO off for its whole lifetime, so plain
//! `stdin().read_line` would show nothing while the user types. Reedline runs in
//! raw mode and draws the typed text itself, which is why every answer goes through it.

use chrono::{DateTime, NaiveDate};
use colored::*;
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema};
use rmcp::ErrorData;
use serde_json::{Map, Number, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::sync::{watch, Mutex, OwnedMutexGuard};

const DATE_FORMAT: &str = "%Y-%m-%d";

/// Number of elicitation prompts that are on screen or queued for the terminal.
static OPEN_PROMPTS: LazyLock<watch::Sender<usize>> = LazyLock::new(|| watch::Sender::new(0));

/// Parallel tool calls can elicit at the same time; the terminal serves one prompt at a time.
static TERMINAL: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

/// Set once by the interactive CLI session, which owns the terminal for the life of the process.
/// An explicit opt-in rather than a tty check: `cargo test` from a terminal inherits a tty stdin.
static TERMINAL_PROMPTS: AtomicBool = AtomicBool::new(false);

/// Called by the interactive CLI session: a person is at this terminal.
pub fn enable_terminal_prompts() {
	TERMINAL_PROMPTS.store(true, Ordering::Relaxed);
}

/// True when a person at this terminal can answer an elicitation.
pub fn can_prompt() -> bool {
	TERMINAL_PROMPTS.load(Ordering::Relaxed) && !crate::agent::inputs::is_non_interactive()
}

/// Live count of elicitation prompts that are on screen or waiting for the terminal.
/// Tool-call idle timers pause while it is above zero.
pub fn subscribe_open_prompts() -> watch::Receiver<usize> {
	OPEN_PROMPTS.subscribe()
}

/// Ask the user. Schema outside the spec -> `invalid_params`; terminal/IO/join failure -> `internal_error`.
pub async fn ask(server_name: &str, request: ElicitRequestParams) -> Result<ElicitResult, ErrorData> {
	// Validate before any UI so a bad schema never touches the terminal.
	let prompt = Prompt::plan(request)?;
	let open = OpenPromptGuard::new();
	let terminal = TERMINAL.clone().lock_owned().await;

	let animation = crate::session::chat::get_animation_manager();
	let mut session = PromptSession {
		_open: open,
		_terminal: terminal,
		resume_spinner: false,
	};
	// Set before the await: if this future is dropped inside `suspend`, Drop still resumes the spinner.
	session.resume_spinner = !animation.is_suspended();
	if session.resume_spinner {
		animation.suspend().await;
	}
	crate::utils::term_echo::drain_stdin();

	let server_name = server_name.to_string();
	// The session moves into the blocking closure: a dropped `ask` future cannot stop the
	// blocking read, so the terminal lock and spinner state must outlive it.
	tokio::task::spawn_blocking(move || {
		let _session = session;
		prompt.run(&server_name)
	})
	.await
	.map_err(|e| ErrorData::internal_error(format!("elicitation prompt task failed: {}", e), None))?
}

/// Counts one prompt in `OPEN_PROMPTS` for as long as it lives.
pub(crate) struct OpenPromptGuard;

impl OpenPromptGuard {
	pub(crate) fn new() -> Self {
		OPEN_PROMPTS.send_modify(|count| *count += 1);
		Self
	}
}

impl Drop for OpenPromptGuard {
	fn drop(&mut self) {
		OPEN_PROMPTS.send_modify(|count| *count -= 1);
	}
}

/// Everything a prompt holds on the shared terminal; released together when the blocking read ends.
struct PromptSession {
	_open: OpenPromptGuard,
	_terminal: OwnedMutexGuard<()>,
	/// Only the prompt that suspended the spinner may resume it.
	resume_spinner: bool,
}

impl Drop for PromptSession {
	fn drop(&mut self) {
		if self.resume_spinner {
			crate::session::chat::get_animation_manager().resume();
		}
	}
}

#[derive(Debug, Clone, PartialEq)]
struct Choice {
	value: String,
	title: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TextFormat {
	Plain,
	Email,
	Uri,
	Date,
	DateTime,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
	Boolean,
	Integer {
		min: Option<i64>,
		max: Option<i64>,
	},
	Number {
		min: Option<f64>,
		max: Option<f64>,
	},
	Text {
		min_len: Option<usize>,
		max_len: Option<usize>,
		format: TextFormat,
	},
	Choice(Vec<Choice>),
	MultiChoice {
		options: Vec<Choice>,
		min_items: Option<usize>,
		max_items: Option<usize>,
	},
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
	key: String,
	label: String,
	description: Option<String>,
	required: bool,
	default: Option<Value>,
	kind: Kind,
}

/// What to show, decided from the request before the terminal is touched.
enum Prompt {
	Form { message: String, fields: Vec<Field> },
	Url { message: String, url: String, host: String },
}

impl Prompt {
	fn plan(request: ElicitRequestParams) -> Result<Self, ErrorData> {
		match request {
			ElicitRequestParams::FormElicitationParams {
				message,
				requested_schema,
				..
			} => {
				let fields = fields(&requested_schema).map_err(|e| ErrorData::invalid_params(e, None))?;
				Ok(Self::Form { message, fields })
			}
			ElicitRequestParams::UrlElicitationParams { message, url, .. } => {
				let parsed = url::Url::parse(&url).map_err(|e| {
					ErrorData::invalid_params(format!("elicitation url '{}' is invalid: {}", url, e), None)
				})?;
				let host = parsed
					.host_str()
					.ok_or_else(|| ErrorData::invalid_params(format!("elicitation url '{}' has no host", url), None))?
					.to_string();
				Ok(Self::Url { message, url, host })
			}
			_ => Err(ErrorData::invalid_params("unsupported elicitation mode", None)),
		}
	}

	fn run(self, server_name: &str) -> Result<ElicitResult, ErrorData> {
		let mut terminal = Terminal {
			editor: Reedline::create(),
		};
		match self {
			Self::Form { message, fields } => {
				print_header(server_name, &message);
				terminal.collect_form(&fields)
			}
			Self::Url { message, url, host } => {
				print_header(server_name, &message);
				eprintln!("{}", url.cyan());
				eprintln!("Domain: {}", host.bold());
				terminal.confirm_open(&url)
			}
		}
	}
}

enum Reply {
	Line(String),
	/// Ctrl+C, Ctrl+D or any other reedline signal.
	Interrupted,
}

struct Terminal {
	editor: Reedline,
}

impl Terminal {
	fn read(&mut self, label: &str) -> Result<Reply, ErrorData> {
		let prompt = DefaultPrompt::new(
			DefaultPromptSegment::Basic(label.to_string()),
			DefaultPromptSegment::Empty,
		);
		match self.editor.read_line(&prompt) {
			Ok(Signal::Success(line)) => Ok(Reply::Line(line)),
			Ok(_) => Ok(Reply::Interrupted),
			Err(e) => Err(ErrorData::internal_error(
				format!("failed to read elicitation answer: {}", e),
				None,
			)),
		}
	}

	/// `None` when the user interrupted.
	fn confirm(&mut self, question: &str, default_yes: bool) -> Result<Option<bool>, ErrorData> {
		loop {
			let Reply::Line(line) = self.read(question)? else {
				return Ok(None);
			};
			let input = line.trim();
			if input.is_empty() {
				return Ok(Some(default_yes));
			}
			match parse_yes_no(input) {
				Some(answer) => return Ok(Some(answer)),
				None => eprintln!("{}", "Answer y or n.".red()),
			}
		}
	}

	fn collect_form(&mut self, fields: &[Field]) -> Result<ElicitResult, ErrorData> {
		let mut content = Map::new();
		for field in fields {
			print_field_help(field);
			let label = if field.required {
				format!("{} *", field.label)
			} else {
				field.label.clone()
			};
			loop {
				let Reply::Line(line) = self.read(&label)? else {
					return Ok(ElicitResult::new(ElicitationAction::Cancel));
				};
				let input = line.trim();
				if input.is_empty() {
					if let Some(default) = &field.default {
						content.insert(field.key.clone(), default.clone());
						break;
					}
					if field.required {
						eprintln!("{}", "This field is required.".red());
						continue;
					}
					break;
				}
				match parse_answer(&field.kind, input) {
					Ok(value) => {
						content.insert(field.key.clone(), value);
						break;
					}
					Err(message) => eprintln!("{}", message.red()),
				}
			}
		}

		match self.confirm("Send? [Y/n]", true)? {
			None => Ok(ElicitResult::new(ElicitationAction::Cancel)),
			Some(false) => Ok(ElicitResult::new(ElicitationAction::Decline)),
			Some(true) => Ok(ElicitResult::new(ElicitationAction::Accept).with_content(Value::Object(content))),
		}
	}

	/// Spec: never open a URL without explicit consent, so the default is no.
	fn confirm_open(&mut self, url: &str) -> Result<ElicitResult, ErrorData> {
		match self.confirm("Open in browser? [y/N]", false)? {
			None => Ok(ElicitResult::new(ElicitationAction::Cancel)),
			Some(false) => Ok(ElicitResult::new(ElicitationAction::Decline)),
			Some(true) => {
				if let Err(e) = open::that(url) {
					eprintln!(
						"{}",
						format!("Could not open the browser ({}). Open the URL manually.", e).red()
					);
				}
				Ok(ElicitResult::new(ElicitationAction::Accept))
			}
		}
	}
}

fn print_header(server_name: &str, message: &str) {
	eprintln!();
	eprintln!("{}", format!("◆ {} asks for input", server_name).bold());
	eprintln!("{}", message);
}

fn print_field_help(field: &Field) {
	if let Some(description) = &field.description {
		eprintln!("{}", description.dimmed());
	}
	if let Kind::Choice(options) | Kind::MultiChoice { options, .. } = &field.kind {
		for (index, option) in options.iter().enumerate() {
			if option.title == option.value {
				eprintln!("  {}) {}", index + 1, option.value);
			} else {
				eprintln!("  {}) {} ({})", index + 1, option.title, option.value);
			}
		}
	}
	let mut hints = Vec::new();
	if let Some(hint) = kind_hint(&field.kind) {
		hints.push(hint);
	}
	if let Some(default) = &field.default {
		hints.push(format!("(default: {})", display_value(default)));
	}
	if !hints.is_empty() {
		eprintln!("{}", hints.join(" ").dimmed());
	}
}

fn kind_hint(kind: &Kind) -> Option<String> {
	match kind {
		Kind::Boolean => Some("[y/n]".to_string()),
		Kind::Integer { min, max } => range_hint("whole number", *min, *max),
		Kind::Number { min, max } => range_hint("number", *min, *max),
		Kind::Text { format, .. } => match format {
			TextFormat::Plain => None,
			TextFormat::Email => Some("email address".to_string()),
			TextFormat::Uri => Some("URI".to_string()),
			TextFormat::Date => Some("date, YYYY-MM-DD".to_string()),
			TextFormat::DateTime => Some("date-time, RFC 3339".to_string()),
		},
		Kind::Choice(_) => Some("number or value".to_string()),
		Kind::MultiChoice { .. } => Some("comma-separated numbers or values".to_string()),
	}
}

fn range_hint<T: std::fmt::Display>(noun: &str, min: Option<T>, max: Option<T>) -> Option<String> {
	match (min, max) {
		(None, None) => None,
		(Some(min), None) => Some(format!("{}, at least {}", noun, min)),
		(None, Some(max)) => Some(format!("{}, at most {}", noun, max)),
		(Some(min), Some(max)) => Some(format!("{}, {} to {}", noun, min, max)),
	}
}

fn display_value(value: &Value) -> String {
	match value {
		Value::String(s) => s.clone(),
		other => other.to_string(),
	}
}

/// Build the ordered field list; `Err` names the property the spec does not cover.
fn fields(schema: &ElicitationSchema) -> Result<Vec<Field>, String> {
	let mut keys: Vec<&String> = Vec::with_capacity(schema.properties.len());
	for key in schema.property_order.iter().flatten() {
		if schema.properties.contains_key(key) && !keys.contains(&key) {
			keys.push(key);
		}
	}
	for key in schema.properties.keys() {
		if !keys.contains(&key) {
			keys.push(key);
		}
	}
	let required = schema.required.as_deref().unwrap_or_default();
	keys.into_iter()
		.map(|key| {
			// Plain JSON Schema keys, not rmcp's non_exhaustive typed variants.
			let definition = serde_json::to_value(&schema.properties[key])
				.map_err(|e| format!("property '{}': cannot read schema: {}", key, e))?;
			field_from_schema(key, &definition, required.contains(key))
		})
		.collect()
}

fn field_from_schema(key: &str, definition: &Value, required: bool) -> Result<Field, String> {
	let unsupported = || format!("property '{}': unsupported schema", key);
	let kind = match definition.get("type").and_then(Value::as_str) {
		Some("boolean") => Kind::Boolean,
		Some("integer") => Kind::Integer {
			min: definition["minimum"].as_i64(),
			max: definition["maximum"].as_i64(),
		},
		Some("number") => Kind::Number {
			min: definition["minimum"].as_f64(),
			max: definition["maximum"].as_f64(),
		},
		Some("string") => match choices(definition, key)? {
			Some(options) => Kind::Choice(options),
			None => Kind::Text {
				min_len: as_usize(&definition["minLength"]),
				max_len: as_usize(&definition["maxLength"]),
				format: text_format(definition),
			},
		},
		Some("array") => {
			let items = &definition["items"];
			Kind::MultiChoice {
				options: choices(items, key)?.ok_or_else(unsupported)?,
				min_items: as_usize(&definition["minItems"]),
				max_items: as_usize(&definition["maxItems"]),
			}
		}
		_ => return Err(unsupported()),
	};
	Ok(Field {
		key: key.to_string(),
		label: definition
			.get("title")
			.and_then(Value::as_str)
			.unwrap_or(key)
			.to_string(),
		description: definition
			.get("description")
			.and_then(Value::as_str)
			.map(str::to_string),
		required,
		default: definition.get("default").cloned(),
		kind,
	})
}

/// Options from `oneOf`/`anyOf: [{const, title}]` or `enum` (+ legacy `enumNames`); `None` when neither is present.
fn choices(schema: &Value, key: &str) -> Result<Option<Vec<Choice>>, String> {
	let titled = ["oneOf", "anyOf"]
		.iter()
		.find_map(|name| schema.get(*name).and_then(Value::as_array));
	if let Some(entries) = titled {
		return entries
			.iter()
			.map(|entry| {
				let value = entry
					.get("const")
					.and_then(Value::as_str)
					.ok_or_else(|| format!("property '{}': option without a string const", key))?;
				let title = entry.get("title").and_then(Value::as_str).unwrap_or(value);
				Ok(Choice {
					value: value.to_string(),
					title: title.to_string(),
				})
			})
			.collect::<Result<Vec<_>, String>>()
			.map(Some);
	}
	let Some(values) = schema.get("enum").and_then(Value::as_array) else {
		return Ok(None);
	};
	let titles = schema.get("enumNames").and_then(Value::as_array);
	values
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			let value = entry
				.as_str()
				.ok_or_else(|| format!("property '{}': enum values must be strings", key))?;
			let title = titles
				.and_then(|names| names.get(index))
				.and_then(Value::as_str)
				.unwrap_or(value);
			Ok(Choice {
				value: value.to_string(),
				title: title.to_string(),
			})
		})
		.collect::<Result<Vec<_>, String>>()
		.map(Some)
}

fn as_usize(value: &Value) -> Option<usize> {
	value.as_u64().and_then(|n| usize::try_from(n).ok())
}

fn text_format(definition: &Value) -> TextFormat {
	match definition.get("format").and_then(Value::as_str) {
		Some("email") => TextFormat::Email,
		Some("uri") => TextFormat::Uri,
		Some("date") => TextFormat::Date,
		Some("date-time") => TextFormat::DateTime,
		_ => TextFormat::Plain,
	}
}

fn parse_yes_no(input: &str) -> Option<bool> {
	match input.trim().to_ascii_lowercase().as_str() {
		"y" | "yes" | "true" => Some(true),
		"n" | "no" | "false" => Some(false),
		_ => None,
	}
}

/// Validate one non-empty answer against the field kind and return the JSON value to send.
fn parse_answer(kind: &Kind, input: &str) -> Result<Value, String> {
	let input = input.trim();
	match kind {
		Kind::Boolean => parse_yes_no(input)
			.map(Value::Bool)
			.ok_or_else(|| "Answer y or n.".to_string()),
		Kind::Integer { min, max } => {
			let number: i64 = input.parse().map_err(|_| "Enter a whole number.".to_string())?;
			check_bounds(number, *min, *max)?;
			Ok(Value::from(number))
		}
		Kind::Number { min, max } => {
			let number: f64 = input.parse().map_err(|_| "Enter a number.".to_string())?;
			check_bounds(number, *min, *max)?;
			Number::from_f64(number)
				.map(Value::Number)
				.ok_or_else(|| "Enter a finite number.".to_string())
		}
		Kind::Text {
			min_len,
			max_len,
			format,
		} => {
			let length = input.chars().count();
			if min_len.is_some_and(|min| length < min) {
				return Err(format!("Must be at least {} characters.", min_len.unwrap_or_default()));
			}
			if max_len.is_some_and(|max| length > max) {
				return Err(format!("Must be at most {} characters.", max_len.unwrap_or_default()));
			}
			validate_format(*format, input)?;
			Ok(Value::String(input.to_string()))
		}
		Kind::Choice(options) => Ok(Value::String(resolve_choice(options, input)?.value.clone())),
		Kind::MultiChoice {
			options,
			min_items,
			max_items,
		} => {
			let mut picked: Vec<Value> = Vec::new();
			for token in input.split(',').map(str::trim).filter(|token| !token.is_empty()) {
				let value = Value::String(resolve_choice(options, token)?.value.clone());
				if !picked.contains(&value) {
					picked.push(value);
				}
			}
			if min_items.is_some_and(|min| picked.len() < min) {
				return Err(format!("Pick at least {}.", min_items.unwrap_or_default()));
			}
			if max_items.is_some_and(|max| picked.len() > max) {
				return Err(format!("Pick at most {}.", max_items.unwrap_or_default()));
			}
			Ok(Value::Array(picked))
		}
	}
}

fn check_bounds<T: PartialOrd + std::fmt::Display + Copy>(
	number: T,
	min: Option<T>,
	max: Option<T>,
) -> Result<(), String> {
	if let Some(min) = min {
		if number < min {
			return Err(format!("Must be at least {}.", min));
		}
	}
	if let Some(max) = max {
		if number > max {
			return Err(format!("Must be at most {}.", max));
		}
	}
	Ok(())
}

fn validate_format(format: TextFormat, input: &str) -> Result<(), String> {
	match format {
		TextFormat::Plain => Ok(()),
		TextFormat::Email => match input.split_once('@') {
			Some((local, domain))
				if !local.is_empty() && !domain.is_empty() && !input.contains(char::is_whitespace) =>
			{
				Ok(())
			}
			_ => Err("Enter an email address like name@example.com.".to_string()),
		},
		TextFormat::Uri => url::Url::parse(input)
			.map(|_| ())
			.map_err(|e| format!("Enter a valid URI: {}.", e)),
		TextFormat::Date => NaiveDate::parse_from_str(input, DATE_FORMAT)
			.map(|_| ())
			.map_err(|_| "Enter a date as YYYY-MM-DD.".to_string()),
		TextFormat::DateTime => DateTime::parse_from_rfc3339(input)
			.map(|_| ())
			.map_err(|_| "Enter a date-time like 2026-01-31T09:30:00Z.".to_string()),
	}
}

/// The exact option value or a 1-based option number. Values win: an enum of
/// numeric strings must not be read as positions.
fn resolve_choice<'a>(options: &'a [Choice], token: &str) -> Result<&'a Choice, String> {
	if let Some(option) = options.iter().find(|option| option.value == token) {
		return Ok(option);
	}
	token
		.parse::<usize>()
		.ok()
		.and_then(|number| number.checked_sub(1))
		.and_then(|index| options.get(index))
		.ok_or_else(|| {
			format!(
				"'{}' is not an option; pick a number from 1 to {} or an option value.",
				token,
				options.len()
			)
		})
}

#[cfg(test)]
#[path = "elicitation_tests.rs"]
mod tests;
