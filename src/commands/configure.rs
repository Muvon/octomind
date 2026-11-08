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

use anyhow::{bail, Context, Result};
use colored::Colorize;
use crossterm::{
	event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
	terminal,
};
use octolib::llm::providers::chatgpt;
use octolib::llm::{ProviderFactory, ProviderSetupInfo};
use octomind::account;
use octomind::branding::{icon_beside, name_and_version};
use octomind::directories::get_config_dir;
use octomind::session::chat::{block_close_ok, block_line, block_open, block_row, key_width};
use octomind::session::picker::pick_index;
use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use toml_edit::{Document, Item, Value};

use super::login::{self, LoginArgs, LoginTarget};

const MODEL_SLOTS: [&[&str]; 3] = [&["model"], &["supervisor", "model"], &["compression", "model"]];
const SLOT_NAMES: [&str; 3] = ["main", "supervisor", "compression"];
const HUB_MODEL: &str = "octohub:auto";
const MODELS_URL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const OTHER_MODEL_LABEL: &str = "other — type a model id that is not in this list";
const KEYS_HINT: &str = "Keys you enter are saved to octomind's own .env file, readable only by you.";
const KEY_SOURCES_HINT: &str = "Keys are read from your shell environment, ./.env, and octomind's own .env.";
const ENV_FILE_NAME: &str = ".env";

/// Where a provider key was found. Startup loads octomind's .env, then ./.env, each overriding the shell.
#[derive(Clone, Copy, Debug, PartialEq)]
enum KeySource {
	Shell,
	OctomindEnvFile,
	ProjectEnvFile,
}

impl KeySource {
	fn label(self) -> &'static str {
		match self {
			KeySource::Shell => "shell environment",
			KeySource::OctomindEnvFile => "octomind's .env",
			KeySource::ProjectEnvFile => "./.env",
		}
	}
}

enum SetupChoice {
	Hub,
	Provider(usize),
	Skip,
}

pub async fn execute(config_path: &Path, first_run: bool) -> Result<()> {
	if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
		bail!("octomind configure requires an interactive terminal; use `octomind config --model provider:model` or edit config.toml at {}", config_path.display());
	}
	let source = match std::fs::read_to_string(config_path) {
		Ok(source) => source,
		Err(e) if e.kind() == io::ErrorKind::NotFound => {
			include_str!("../../config-templates/default.toml").to_string()
		}
		Err(e) => return Err(e).with_context(|| format!("could not read {}", config_path.display())),
	};
	let mut document = Document::parse(source).context("could not parse config.toml")?;
	let providers = providers_for_setup(ProviderFactory::provider_setup_info(), key_source)?;
	let hub_signed_in = env_present(account::HUB_KEY_ENV)?;
	let labels = setup_labels(&providers, hub_signed_in);
	let title = setup_title();
	// Esc is the same as Skip: on first run the defaults still get written, so startup is never blocked.
	let choice = match pick_index(&title, &labels, 0)? {
		Some(index) => setup_choice(index, providers.len()),
		None => SetupChoice::Skip,
	};

	let models = match choice {
		SetupChoice::Hub => {
			login::execute(&LoginArgs {
				target: LoginTarget::Octomind,
				force: !hub_signed_in,
				no_browser: false,
			})
			.await?;
			Some([HUB_MODEL.to_string(), HUB_MODEL.to_string(), HUB_MODEL.to_string()])
		}
		SetupChoice::Provider(i) => {
			let provider = &providers[i].0;
			if provider.name == "chatgpt" {
				login::execute(&LoginArgs {
					target: LoginTarget::Chatgpt,
					force: false,
					no_browser: false,
				})
				.await?;
			}
			for name in provider
				.api_key_env
				.into_iter()
				.chain(provider.extra_env.iter().copied())
			{
				if !env_present(name)? {
					eprintln!("{} needs {name}. {KEYS_HINT}", provider.name);
					let value = read_secret(name)?;
					account::write_env_var(name, &value)?;
					std::env::set_var(name, value);
				}
			}
			let catalog = provider_models(provider).await?;
			let main = choose_model(provider.name, "main", &catalog, None)?;
			let (supervisor, compression) = if confirm(&format!(
				"Use {main} for the supervisor and compression too? They check work and shorten long \
				 conversations; a cheaper model often fits. [Y/n] "
			))? {
				(main.clone(), main.clone())
			} else {
				(
					choose_model(provider.name, "supervisor", &catalog, Some(&main))?,
					choose_model(provider.name, "compression", &catalog, Some(&main))?,
				)
			};
			Some([main, supervisor, compression].map(|model| format!("{}:{model}", provider.name)))
		}
		SetupChoice::Skip => {
			eprintln!("Skipped. Run `octomind configure` when you're ready.");
			None
		}
	};
	if let Some(models) = &models {
		let source = set_model_names(&document, models)?;
		document = Document::parse(source).context("could not parse configured model names")?;
	}
	// First-run startup still creates the default template when setup is skipped.
	if models.is_some() || first_run {
		if let Some(parent) = config_path.parent().filter(|p| !p.as_os_str().is_empty()) {
			std::fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
		}
		std::fs::write(config_path, document.raw())
			.with_context(|| format!("could not write {}", config_path.display()))?;
	}

	block_open("configure", None);
	let kw = key_width(["main", "supervisor", "compression", "provider", "key", "signed in"]);
	for (path, slot) in MODEL_SLOTS.iter().zip(SLOT_NAMES) {
		let mut item = document.as_item();
		for key in *path {
			item = item
				.get(key)
				.with_context(|| format!("missing {} in config.toml", path.join(".")))?;
		}
		let model = item
			.get("name")
			.and_then(Item::as_str)
			.with_context(|| format!("{}.name: string required", path.join(".")))?;
		block_row(slot, model, kw);
		let (provider_name, _) = ProviderFactory::parse_model(model)?;
		let info = ProviderFactory::provider_setup_info()
			.into_iter()
			.find(|info| info.name.eq_ignore_ascii_case(&provider_name))
			.with_context(|| format!("unknown provider: {provider_name}"))?;
		block_row("provider", &provider_name, kw);
		match info.api_key_env {
			Some(name) => {
				let found = match key_source(name)? {
					Some(source) => format!("in {}", source.label()),
					None => format!("missing, set {name}"),
				};
				block_row("key", &found, kw);
			}
			// chatgpt bills the user's ChatGPT plan through a sign-in, not a key.
			None if info.name == "chatgpt" => {
				let signed_in = chatgpt::current_account()?.is_some();
				block_row("signed in", if signed_in { "yes" } else { "no" }, kw);
			}
			// Local servers (ollama, local) need no credential.
			None => {}
		}
	}
	block_line("change later: `octomind configure`, `/model` for one session,");
	block_line(&format!("or edit {}", config_path.display()));
	block_close_ok("configure", None);
	Ok(())
}

fn set_model_names(document: &Document<String>, models: &[String; 3]) -> Result<String> {
	let mut replacements = Vec::new();
	for (path, model) in MODEL_SLOTS.iter().zip(models) {
		let mut item = document.as_item();
		for key in *path {
			item = item
				.get(key)
				.with_context(|| format!("missing {} in config.toml", path.join(".")))?;
		}
		let value = item
			.get("name")
			.and_then(Item::as_value)
			.with_context(|| format!("{}.name: string required", path.join(".")))?;
		if value.as_str().is_none() {
			bail!("{}.name: string required", path.join("."));
		}
		let span = value.span().expect("parsed model names have source spans");
		replacements.push((span, Value::from(model.as_str()).to_string()));
	}
	// Whole-document serialization normalizes line endings; replace only the parsed string tokens.
	replacements.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
	let mut source = document.raw().to_string();
	for (span, replacement) in replacements {
		source.replace_range(span, &replacement);
	}
	Ok(source)
}

fn setup_title() -> String {
	let intro = [
		name_and_version(),
		"Where should octomind get its models?".bold().to_string(),
	];
	// The icon is drawn with color escapes only; without color it would print as bare block glyphs.
	let mut lines = if colored::control::SHOULD_COLORIZE.should_colorize() {
		icon_beside(&intro)
	} else {
		intro.to_vec()
	};
	lines.extend([
		String::new(),
		format!(
			"{} one sign-in, `auto` picks a model for each job, billed from credits you buy.",
			"Octomind hub:".bold()
		),
		format!("{} use a key you already have.", "Your own provider:".bold()),
		KEY_SOURCES_HINT.dimmed().to_string(),
		KEYS_HINT.dimmed().to_string(),
		String::new(),
		"Type to filter, ↑/↓ to move, Enter to choose, Esc to skip."
			.dimmed()
			.to_string(),
	]);
	lines.join("\n")
}

fn providers_for_setup(
	providers: Vec<ProviderSetupInfo>,
	find_key: impl Fn(&str) -> Result<Option<KeySource>>,
) -> Result<Vec<(ProviderSetupInfo, Option<KeySource>)>> {
	let mut providers = providers
		.into_iter()
		.filter(|provider| provider.name != "octohub")
		.map(|provider| {
			let key = match provider.api_key_env {
				Some(name) => find_key(name)?,
				None => None,
			};
			Ok((provider, key))
		})
		.collect::<Result<Vec<_>>>()?;
	providers.sort_by_key(|(_, key)| key.is_none());
	Ok(providers)
}

fn env_present(name: &str) -> Result<bool> {
	match std::env::var(name) {
		Ok(value) => Ok(!value.trim().is_empty()),
		Err(std::env::VarError::NotPresent) => Ok(false),
		Err(std::env::VarError::NotUnicode(_)) => bail!("{name}: value must be valid Unicode"),
	}
}

/// Files are re-read on each call so a key typed during setup is reported from where it was saved.
fn key_source(name: &str) -> Result<Option<KeySource>> {
	if !env_present(name)? {
		return Ok(None);
	}
	// The file loaded last wins, so check it first.
	if env_file_keys(Path::new(ENV_FILE_NAME))?.contains(name) {
		return Ok(Some(KeySource::ProjectEnvFile));
	}
	if env_file_keys(&get_config_dir()?.join(ENV_FILE_NAME))?.contains(name) {
		return Ok(Some(KeySource::OctomindEnvFile));
	}
	Ok(Some(KeySource::Shell))
}

/// Names a dotenv file sets to a non-empty value; a missing file sets none.
fn env_file_keys(path: &Path) -> Result<HashSet<String>> {
	let entries = match dotenvy::from_path_iter(path) {
		Ok(entries) => entries,
		Err(dotenvy::Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
		Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
	};
	let mut keys = HashSet::new();
	for entry in entries {
		let (name, value) = entry.with_context(|| format!("could not parse {}", path.display()))?;
		if !value.trim().is_empty() {
			keys.insert(name);
		}
	}
	Ok(keys)
}

fn setup_labels(providers: &[(ProviderSetupInfo, Option<KeySource>)], hub_signed_in: bool) -> Vec<String> {
	let hub_status = if hub_signed_in {
		"signed in".green()
	} else {
		"sign in, `auto` models, prepaid credits".normal()
	};
	let mut labels = vec![format!(
		"{} {} — {hub_status}",
		"Octomind hub".bold(),
		"(recommended)".bright_magenta()
	)];
	labels.extend(providers.iter().map(|(provider, key)| {
		let name = provider.name.bold();
		match (provider.api_key_env, key) {
			(Some(_), Some(source)) => format!("{name} — {}", format!("key in {}", source.label()).green()),
			(Some(env), None) => format!("{name} — {}", format!("needs {env}").dimmed()),
			(None, _) if provider.name == "chatgpt" => {
				format!("{name} — {}", "sign in with your ChatGPT plan".cyan())
			}
			(None, _) => name.to_string(),
		}
	}));
	labels.push("Skip for now — keep the current models".dimmed().to_string());
	labels
}

/// Map a row of the first screen to its meaning: hub first, providers, skip last.
fn setup_choice(index: usize, provider_count: usize) -> SetupChoice {
	match index {
		0 => SetupChoice::Hub,
		i if i <= provider_count => SetupChoice::Provider(i - 1),
		_ => SetupChoice::Skip,
	}
}

async fn provider_models(provider: &ProviderSetupInfo) -> Result<Vec<String>> {
	let Some(url) = provider.models_url else {
		return Ok(provider.models.clone());
	};
	eprintln!("Loading {}'s model list…", provider.name);
	match public_models(url).await {
		Ok(models) => Ok(models),
		// The list is a convenience: typing the id still configures the provider correctly.
		Err(e) => {
			eprintln!(
				"Could not load {}'s model list ({e:#}); type the model id instead.",
				provider.name
			);
			Ok(Vec::new())
		}
	}
}

async fn public_models(url: &str) -> Result<Vec<String>> {
	#[derive(serde::Deserialize)]
	struct Catalog {
		data: Vec<Entry>,
	}
	#[derive(serde::Deserialize)]
	struct Entry {
		id: String,
		#[serde(default)]
		created: i64,
		supported_parameters: Option<Vec<String>>,
	}
	let mut catalog: Catalog = reqwest::Client::new()
		.get(url)
		.timeout(MODELS_URL_TIMEOUT)
		.send()
		.await?
		.error_for_status()?
		.json()
		.await?;
	// Newest first: a fresh setup most likely wants a current model.
	catalog.data.sort_by_key(|entry| std::cmp::Reverse(entry.created));
	// Octomind's agents work through tool calls; a catalog that says a model has no tools is no use here.
	Ok(catalog
		.data
		.into_iter()
		.filter(|entry| match &entry.supported_parameters {
			Some(params) => params.iter().any(|p| p == "tools"),
			None => true,
		})
		.map(|entry| entry.id)
		.collect())
}

fn choose_model(provider: &str, slot: &str, models: &[String], default: Option<&str>) -> Result<String> {
	if !models.is_empty() {
		let mut labels = models.to_vec();
		labels.push(OTHER_MODEL_LABEL.to_string());
		let selected = default
			.and_then(|model| models.iter().position(|m| m == model))
			.unwrap_or(0);
		let title = format!(
			"Choose the {slot} model on {provider}.\n\n\
			 Type to filter, ↑/↓ to move, Enter to choose, Esc to cancel."
		);
		let index =
			pick_index(&title, &labels, selected)?.with_context(|| format!("{slot} model selection cancelled"))?;
		if index < models.len() {
			return Ok(models[index].clone());
		}
	}
	let mut stderr = io::stderr();
	match default {
		Some(model) => write!(stderr, "{slot} model id on {provider} [{model}] (Enter keeps it): ")?,
		None => write!(
			stderr,
			"{slot} model id on {provider}, exactly as {provider} names it: "
		)?,
	}
	stderr.flush()?;
	let mut value = String::new();
	if io::stdin().read_line(&mut value)? == 0 {
		bail!("{slot} model: input ended");
	}
	let value = value.trim();
	match (value.is_empty(), default) {
		(true, Some(model)) => Ok(model.to_string()),
		(true, None) => bail!("{slot} model: a non-empty model id is required"),
		(false, _) => Ok(value.to_string()),
	}
}

/// Yes/no question on stderr; Enter means yes.
fn confirm(question: &str) -> Result<bool> {
	let mut stderr = io::stderr();
	write!(stderr, "{question}")?;
	stderr.flush()?;
	let mut answer = String::new();
	if io::stdin().read_line(&mut answer)? == 0 {
		bail!("input ended");
	}
	parse_yes(&answer)
}

fn parse_yes(answer: &str) -> Result<bool> {
	match answer.trim().to_ascii_lowercase().as_str() {
		"" | "y" | "yes" => Ok(true),
		"n" | "no" => Ok(false),
		other => bail!("answer y or n, got `{other}`"),
	}
}

fn read_secret(name: &str) -> Result<String> {
	terminal::enable_raw_mode()?;
	let result = prompt_secret(name, &mut io::stderr(), event::read);
	terminal::disable_raw_mode()?;
	eprintln!();
	result
}

fn prompt_secret(
	name: &str,
	output: &mut impl Write,
	mut read_event: impl FnMut() -> io::Result<Event>,
) -> Result<String> {
	loop {
		write!(output, "{name} (hidden): ")?;
		output.flush()?;
		let mut value = String::new();
		loop {
			if let Event::Key(key) = read_event()? {
				if key.kind == KeyEventKind::Release {
					continue;
				}
				match key.code {
					KeyCode::Enter => {
						if value.trim().is_empty() {
							write!(output, "\r\n{name}: a non-empty value is required\r\n")?;
							break;
						}
						return Ok(value);
					}
					KeyCode::Esc => bail!("{name}: input cancelled"),
					KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
						bail!("{name}: input cancelled");
					}
					KeyCode::Backspace => {
						value.pop();
					}
					KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => value.push(c),
					_ => {}
				}
			}
		}
	}
}

#[cfg(test)]
#[path = "configure_tests.rs"]
mod tests;
