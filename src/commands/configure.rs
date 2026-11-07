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
use crossterm::{
	event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
	terminal,
};
use octolib::llm::{ProviderFactory, ProviderSetupInfo};
use octomind::account;
use octomind::session::chat::{block_close_ok, block_line, block_open, block_row, key_width};
use octomind::session::picker::pick_index;
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
	let providers = providers_for_setup(ProviderFactory::provider_setup_info(), env_present)?;
	let hub_signed_in = env_present(account::HUB_KEY_ENV)?;
	let labels = setup_labels(&providers, hub_signed_in);
	let title = format!(
		"Where should octomind get its models?\n\n\
		 Octomind hub: one sign-in, `auto` picks a model for each job, billed from credits you buy.\n\
		 Your own provider: use a key you already have.\n\
		 {KEYS_HINT}\n\n\
		 Type to filter, ↑/↓ to move, Enter to choose, Esc to cancel."
	);
	let index = pick_index(&title, &labels, 0)?.context("setup cancelled; run `octomind configure` to try again")?;

	let models = match setup_choice(index, providers.len()) {
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
	let kw = key_width(["main", "supervisor", "compression", "provider", "key present"]);
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
		let has_key = match info.api_key_env {
			Some(name) => env_present(name)?,
			None => false,
		};
		block_row("provider", &provider_name, kw);
		block_row("key present", if has_key { "yes" } else { "no" }, kw);
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

fn providers_for_setup(
	providers: Vec<ProviderSetupInfo>,
	has_env: impl Fn(&str) -> Result<bool>,
) -> Result<Vec<(ProviderSetupInfo, bool)>> {
	let mut providers = providers
		.into_iter()
		.filter(|provider| provider.name != "octohub")
		.map(|provider| {
			let key_found = match provider.api_key_env {
				Some(name) => has_env(name)?,
				None => false,
			};
			Ok((provider, key_found))
		})
		.collect::<Result<Vec<_>>>()?;
	providers.sort_by_key(|(_, key_found)| !key_found);
	Ok(providers)
}

fn env_present(name: &str) -> Result<bool> {
	match std::env::var(name) {
		Ok(value) => Ok(!value.trim().is_empty()),
		Err(std::env::VarError::NotPresent) => Ok(false),
		Err(std::env::VarError::NotUnicode(_)) => bail!("{name}: value must be valid Unicode"),
	}
}

fn setup_labels(providers: &[(ProviderSetupInfo, bool)], hub_signed_in: bool) -> Vec<String> {
	let hub = if hub_signed_in {
		"Octomind hub (recommended) — signed in"
	} else {
		"Octomind hub (recommended) — sign in, `auto` models, prepaid credits"
	};
	let mut labels = vec![hub.to_string()];
	labels.extend(
		providers
			.iter()
			.map(|(provider, key_found)| match (provider.api_key_env, key_found) {
				(Some(_), true) => format!("{} — key found", provider.name),
				(Some(env), false) => format!("{} — needs {env}", provider.name),
				(None, _) if provider.name == "chatgpt" => "chatgpt — sign in with your ChatGPT plan".to_string(),
				(None, _) => provider.name.to_string(),
			}),
	);
	labels.push("Skip for now — keep the current models".to_string());
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
	Ok(catalog.data.into_iter().map(|entry| entry.id).collect())
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
