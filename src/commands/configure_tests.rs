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

#[test]
fn configure_changes_only_three_names_and_preserves_comments() {
	let source = "# keep the header\n\
		telemetry = false # keep root settings\n\n\
		[model] # main profile\n\
		# keep the main comment\n\
		name   =   \"old:main\" # main inline\n\
		temperature = 0.7\n\n\
		[supervisor.model]\n\
		name = 'old:supervisor' # supervisor inline\n\
		max_tokens = 1234\n\n\
		[compression.model]\n\
		name = \"old:compression\" # compression inline\n\
		reasoning_effort = \"none\"\n\n\
		[[roles]]\n\
		name = \"assistant\"\n\
		[roles.model]\n\
		name = \"leave:this-model\"\n";
	for source in [source.to_string(), source.replace('\n', "\r\n")] {
		let document = Document::parse(source.clone()).expect("parse config");
		let after = set_model_names(
			&document,
			&[
				"openai:main".into(),
				"openai:supervisor".into(),
				"openai:compression".into(),
			],
		)
		.expect("edit names");
		let expected = source
			.replace("\"old:main\"", "\"openai:main\"")
			.replace("'old:supervisor'", "\"openai:supervisor\"")
			.replace("\"old:compression\"", "\"openai:compression\"");
		assert_eq!(after, expected);
	}
}

#[test]
fn configure_template_changes_only_the_three_model_names() {
	let source = include_str!("../../config-templates/default.toml");
	let document = Document::parse(source.to_string()).expect("parse embedded template");
	let after = set_model_names(
		&document,
		&[
			"openai:main".into(),
			"openai:supervisor".into(),
			"openai:compression".into(),
		],
	)
	.expect("edit template");
	let differences: Vec<_> = source
		.lines()
		.zip(after.lines())
		.filter(|(before, after)| before != after)
		.collect();
	assert_eq!(source.lines().count(), after.lines().count());
	assert_eq!(differences.len(), MODEL_SLOTS.len());
	assert!(differences
		.iter()
		.all(|(before, _)| *before == "name = \"octohub:auto\""));
	for model in ["openai:main", "openai:supervisor", "openai:compression"] {
		assert!(after.contains(&format!("name = \"{model}\"")));
	}
}

#[test]
fn configure_rejects_missing_or_malformed_model_names() {
	for source in ["[model]\n", "[model]\nname = 42\n"] {
		let document = Document::parse(source.to_string()).expect("parse config");
		let err = set_model_names(&document, &["openai:a".into(), "openai:b".into(), "openai:c".into()])
			.expect_err("required name must not be created or coerced");
		assert!(err.to_string().contains("model.name"), "{err}");
	}
}

#[test]
fn configure_orders_key_found_providers_first_and_excludes_hub() {
	let provider = |name, api_key_env| ProviderSetupInfo {
		name,
		api_key_env,
		extra_env: &[],
		models: vec![],
		models_url: None,
	};
	let providers = providers_for_setup(
		vec![
			provider("without-key", Some("MISSING_KEY")),
			provider("octohub", Some("FOUND_KEY")),
			provider("first-with-key", Some("FOUND_KEY")),
			provider("no-key-needed", None),
			provider("second-with-key", Some("FOUND_KEY")),
		],
		|name| Ok((name == "FOUND_KEY").then_some(KeySource::Shell)),
	)
	.expect("provider metadata");
	let labels: Vec<_> = providers.iter().map(|(info, found)| (info.name, *found)).collect();
	assert_eq!(
		labels,
		[
			("first-with-key", Some(KeySource::Shell)),
			("second-with-key", Some(KeySource::Shell)),
			("without-key", None),
			("no-key-needed", None),
		]
	);
}

#[test]
fn configure_propagates_environment_errors() {
	let providers = vec![ProviderSetupInfo {
		name: "openai",
		api_key_env: Some("OPENAI_API_KEY"),
		extra_env: &[],
		models: vec![],
		models_url: None,
	}];
	let err = providers_for_setup(providers, |name| bail!("{name}: invalid value"))
		.expect_err("environment errors must surface");
	assert_eq!(err.to_string(), "OPENAI_API_KEY: invalid value");
}

#[test]
fn configure_secret_reprompts_empty_input_without_echoing_values() {
	for name in ["PROVIDER_API_KEY", "PROVIDER_REQUIRED_EXTRA"] {
		let mut events = [
			KeyCode::Enter,
			KeyCode::Char(' '),
			KeyCode::Enter,
			KeyCode::Char('s'),
			KeyCode::Char('e'),
			KeyCode::Char('c'),
			KeyCode::Char('r'),
			KeyCode::Char('e'),
			KeyCode::Char('t'),
			KeyCode::Enter,
		]
		.into_iter();
		let mut output = Vec::new();
		let value = prompt_secret(name, &mut output, || {
			Ok(Event::Key(crossterm::event::KeyEvent::new(
				events.next().expect("input must finish with a non-empty value"),
				KeyModifiers::NONE,
			)))
		})
		.expect("required value entered");
		assert_eq!(value.as_deref(), Some("secret"));
		assert!(events.next().is_none());
		assert_eq!(
			String::from_utf8(output).unwrap(),
			format!(
				"{name} (hidden, Esc to go back): \r\n{name}: a non-empty value is required\r\n\
			{name} (hidden, Esc to go back): \r\n{name}: a non-empty value is required\r\n\
			{name} (hidden, Esc to go back): "
			)
		);
	}
}

#[test]
fn configure_secret_trims_surrounding_whitespace() {
	let mut events = [
		KeyCode::Char(' '),
		KeyCode::Char('a'),
		KeyCode::Char('b'),
		KeyCode::Char(' '),
		KeyCode::Enter,
	]
	.into_iter();
	let mut output = Vec::new();
	let value = prompt_secret("PROVIDER_API_KEY", &mut output, || {
		Ok(Event::Key(crossterm::event::KeyEvent::new(
			events.next().expect("input must finish with Enter"),
			KeyModifiers::NONE,
		)))
	})
	.expect("value entered");
	assert_eq!(value.as_deref(), Some("ab"));
	assert!(events.next().is_none());
	assert_eq!(
		String::from_utf8(output).unwrap(),
		"PROVIDER_API_KEY (hidden, Esc to go back): "
	);
}

#[test]
fn configure_secret_esc_goes_back() {
	let mut output = Vec::new();
	let value = prompt_secret("PROVIDER_API_KEY", &mut output, || {
		Ok(Event::Key(crossterm::event::KeyEvent::new(
			KeyCode::Esc,
			KeyModifiers::NONE,
		)))
	})
	.expect("Esc is not an error");
	assert_eq!(value, None);
}

#[test]
fn configure_secret_propagates_input_errors() {
	let mut output = Vec::new();
	let err = prompt_secret("PROVIDER_REQUIRED_EXTRA", &mut output, || {
		Err(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal input ended"))
	})
	.expect_err("input failure must end setup");
	assert_eq!(err.to_string(), "terminal input ended");
}

#[test]
fn configure_lists_hub_first_providers_then_skip() {
	let provider = |name, api_key_env| ProviderSetupInfo {
		name,
		api_key_env,
		extra_env: &[],
		models: vec![],
		models_url: None,
	};
	let providers = vec![
		(
			provider("openrouter", Some("OPENROUTER_API_KEY")),
			Some(KeySource::Shell),
		),
		(
			provider("openai", Some("OPENAI_API_KEY")),
			Some(KeySource::OctomindEnvFile),
		),
		(
			provider("deepseek", Some("DEEPSEEK_API_KEY")),
			Some(KeySource::ProjectEnvFile),
		),
		(provider("anthropic", Some("ANTHROPIC_API_KEY")), None),
		(provider("chatgpt", None), None),
	];
	let plain = |labels: Vec<String>| -> Vec<String> {
		labels
			.iter()
			.map(|label| octomind::session::chat::strip_ansi(label))
			.collect()
	};
	assert_eq!(
		plain(setup_labels(&providers, false)),
		[
			"Octomind hub (recommended) — sign in, `auto` models, prepaid credits",
			"openrouter — key in shell environment",
			"openai — key in octomind's .env",
			"deepseek — key in ./.env",
			"anthropic — needs ANTHROPIC_API_KEY",
			"chatgpt — sign in with your ChatGPT plan",
			"Skip for now — keep the current models",
		]
	);
	assert_eq!(
		plain(setup_labels(&providers, true))[0],
		"Octomind hub (recommended) — signed in"
	);
	assert!(matches!(setup_choice(0, providers.len()), SetupChoice::Hub));
	assert!(matches!(setup_choice(1, providers.len()), SetupChoice::Provider(0)));
	assert!(matches!(setup_choice(3, providers.len()), SetupChoice::Provider(2)));
	assert!(matches!(
		setup_choice(providers.len() + 1, providers.len()),
		SetupChoice::Skip
	));
}

#[test]
fn configure_yes_no_defaults_to_yes_and_rejects_other_answers() {
	for yes in ["\n", "y\n", "YES\n", " yes "] {
		assert!(parse_yes(yes).unwrap(), "{yes:?}");
	}
	for no in ["n\n", "No\n"] {
		assert!(!parse_yes(no).unwrap(), "{no:?}");
	}
	assert!(parse_yes("maybe\n").is_err());
}
