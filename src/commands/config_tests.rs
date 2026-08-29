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

//! `octomind config` command coverage: read-only paths (list-themes / show /
//! validate) render against the template config; malformed inputs exercise
//! every early-error branch; setter paths run under a sandboxed
//! `OCTOMIND_DATA_DIR` so they write a throwaway config file that is then
//! reloaded to prove the save round-trip.

use super::*;
use octomind::config::LogLevel;
use serial_test::serial;
use std::path::PathBuf;

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop — a failed assert must not leak
/// a sandboxed data dir into the next test.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

/// A fresh per-test data dir under the system temp dir.
fn sandbox(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-cfg-{tag}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

/// Where `Config::save()` writes inside a sandboxed data dir.
fn saved_config_path(dir: &std::path::Path) -> PathBuf {
	dir.join("config").join("config.toml")
}

fn args() -> ConfigArgs {
	ConfigArgs {
		model: None,
		api_key: None,
		log_level: None,
		mcp_providers: None,
		mcp_server: None,
		system: None,
		markdown_enable: None,
		markdown_theme: None,
		list_themes: false,
		show: false,
		validate: false,
		upgrade: false,
	}
}

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

// ── read-only paths ─────────────────────────────────────────────────────────

#[test]
fn test_list_themes_renders() {
	let mut a = args();
	a.list_themes = true;
	execute(&a, template_config()).expect("list themes is read-only");
}

#[test]
fn test_show_configuration_renders() {
	let mut a = args();
	a.show = true;
	execute(&a, template_config()).expect("show is read-only");
}

#[test]
fn test_validate_template_config() {
	let mut a = args();
	a.validate = true;
	execute(&a, template_config()).expect("template config must validate");
}

#[test]
fn test_show_configuration_renders_external_servers() {
	let mut config = template_config();
	config.mcp.servers.push(McpServerConfig::http(
		"remote",
		"https://example.com/mcp",
		45,
		vec!["tool_a".to_string()],
	));
	config.mcp.servers.push(McpServerConfig::stdin(
		"local",
		"node",
		vec!["server.js".to_string()],
		30,
		vec![],
	));
	config
		.mcp
		.servers
		.push(McpServerConfig::builtin("custom", 30, vec![]));
	show_configuration(&config).expect("show renders external server details");
}

#[test]
fn test_validate_invalid_config_returns_error() {
	let mut config = template_config();
	config.model.clear();
	let mut a = args();
	a.validate = true;
	let err = execute(&a, config).expect_err("empty model must fail validation");
	assert!(err.to_string().contains("Model field cannot be empty"));
}

// ── early-error branches (each returns before any save) ─────────────────────

#[test]
#[serial]
fn test_model_without_provider_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("model-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.model = Some("no-colon-here".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"rejected model must not save"
	);
}

#[test]
#[serial]
fn test_api_key_without_colon_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("apikey-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.api_key = Some("openrouter".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"rejected api key must not save"
	);
}

#[test]
#[serial]
fn test_api_key_directs_to_environment_variable() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("apikey-env");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.api_key = Some("openrouter:secret-value".to_string());
	execute(&a, template_config()).expect("env-var guidance is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"api key must never be written"
	);
}

#[test]
#[serial]
fn test_invalid_log_level_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("loglevel-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.log_level = Some("verbose".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"rejected log level must not save"
	);
}

#[test]
#[serial]
fn test_invalid_theme_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("theme-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.markdown_theme = Some("neon-pink".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"rejected theme must not save"
	);
}

#[test]
#[serial]
fn test_mcp_server_without_config_parts_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.mcp_server = Some("lonely".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"rejected mcp server must not save"
	);
}

#[test]
#[serial]
fn test_http_mcp_server_without_url_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-nourl");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.mcp_server = Some("web,type=http".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"url-less http server must not save"
	);
}

#[test]
#[serial]
fn test_stdio_mcp_server_without_command_is_rejected() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-nocmd");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.mcp_server = Some("local,type=stdio".to_string());
	execute(&a, template_config()).expect("rejected input is not a hard error");
	assert!(
		!saved_config_path(&dir).exists(),
		"command-less stdio server must not save"
	);
}

// ── setter paths under a sandboxed data dir ─────────────────────────────────

#[test]
#[serial]
fn test_set_model_persists_and_roundtrips() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("model-ok");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.model = Some("openrouter:anthropic/claude-3.5-sonnet".to_string());
	execute(&a, template_config()).expect("set model saves");

	let path = saved_config_path(&dir);
	assert!(path.exists(), "config file must be written");
	let reloaded = Config::load_from_path(&path).expect("saved config reloads");
	assert_eq!(reloaded.model, "openrouter:anthropic/claude-3.5-sonnet");
}

#[test]
#[serial]
fn test_set_log_level_persists() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("loglevel-ok");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.log_level = Some("debug".to_string());
	execute(&a, template_config()).expect("set log level saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	assert_eq!(reloaded.log_level, LogLevel::Debug);
}

#[test]
#[serial]
fn test_disable_markdown_persists() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("markdown-off");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.markdown_enable = Some(false);
	execute(&a, template_config()).expect("disable markdown saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	assert!(!reloaded.enable_markdown_rendering);
}

#[test]
#[serial]
fn test_set_valid_theme_persists() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("theme-ok");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.markdown_theme = Some("ocean".to_string());
	execute(&a, template_config()).expect("set theme saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	assert_eq!(reloaded.markdown_theme, "ocean");
}

#[test]
#[serial]
fn test_system_prompt_set_then_reset_to_default() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("system");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.system = Some("You are terse.".to_string());
	execute(&a, template_config()).expect("set system prompt saves");
	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	assert_eq!(reloaded.system.as_deref(), Some("You are terse."));

	let mut reset = args();
	reset.system = Some("DEFAULT".to_string());
	execute(&reset, template_config()).expect("reset system prompt saves");
	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	assert_eq!(reloaded.system, None);
}

#[test]
#[serial]
fn test_mcp_providers_replace_servers_with_dedup() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpproviders");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.mcp_providers = Some("core, core, runtime, agent, orchestration".to_string());
	execute(&a, template_config()).expect("set mcp providers saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	let names: Vec<&str> = reloaded.mcp.servers.iter().map(|s| s.name()).collect();
	assert_eq!(names, vec!["core", "runtime", "agent", "orchestration"]);
	assert!(reloaded
		.mcp
		.servers
		.iter()
		.all(|s| matches!(s, McpServerConfig::Builtin { .. })));
}

#[test]
#[serial]
fn test_http_mcp_server_persists_with_timeout() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-http");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.mcp_server = Some("web,url=http://127.0.0.1:9/mcp,timeout=45".to_string());
	execute(&a, template_config()).expect("add http server saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	let srv = reloaded
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "web")
		.expect("web saved");
	assert!(matches!(srv, McpServerConfig::Http { .. }));
	assert_eq!(srv.url(), Some("http://127.0.0.1:9/mcp"));
	assert_eq!(srv.timeout_seconds(), 45);
}

#[test]
#[serial]
fn test_stdio_mcp_server_persists_with_args() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-stdio");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.mcp_server = Some(
		"local,type=stdio,command=node,args=server.js --verbose,timeout_seconds=20".to_string(),
	);
	execute(&a, template_config()).expect("add stdio server saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	let srv = reloaded
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "local")
		.expect("local saved");
	let McpServerConfig::Stdin { command, args, .. } = srv else {
		panic!("expected stdio server, got {srv:?}");
	};
	assert_eq!(command, "node");
	assert_eq!(
		args,
		&vec!["server.js".to_string(), "--verbose".to_string()]
	);
	assert_eq!(srv.timeout_seconds(), 20);
}

#[test]
#[serial]
fn test_builtin_mcp_server_persists() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-builtin");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.mcp_server = Some("extra,type=builtin".to_string());
	execute(&a, template_config()).expect("add builtin server saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	let srv = reloaded
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "extra")
		.expect("extra saved");
	assert!(matches!(srv, McpServerConfig::Builtin { .. }));
	assert_eq!(srv.timeout_seconds(), DEFAULT_MCP_TIMEOUT_SECONDS);
}

#[test]
#[serial]
fn test_mcp_server_unknown_keys_warn_but_save() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-warn");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.mcp_server =
		Some("warnsrv,url=http://127.0.0.1:9/mcp,type=funky,timeout=abc,flavor=sweet".to_string());
	execute(&a, template_config()).expect("unknown keys warn but still save");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	let srv = reloaded
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "warnsrv")
		.expect("saved");
	assert!(
		matches!(srv, McpServerConfig::Http { .. }),
		"unknown type defaults to HTTP"
	);
	assert_eq!(
		srv.timeout_seconds(),
		DEFAULT_MCP_TIMEOUT_SECONDS,
		"invalid timeout falls back"
	);
}

#[test]
#[serial]
fn test_mcp_server_replaces_same_name() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("mcpsrv-replace");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let mut a = args();
	a.mcp_server = Some("core,url=http://127.0.0.1:9/mcp".to_string());
	execute(&a, template_config()).expect("replacing a server saves");

	let reloaded = Config::load_from_path(&saved_config_path(&dir)).expect("reload");
	let cores: Vec<_> = reloaded
		.mcp
		.servers
		.iter()
		.filter(|s| s.name() == "core")
		.collect();
	assert_eq!(
		cores.len(),
		1,
		"same-name server must be replaced, not duplicated"
	);
	assert!(matches!(cores[0], McpServerConfig::Http { .. }));
}

#[test]
#[serial]
fn test_no_changes_reports_or_creates_config_file() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	// Existing file: reported as "no changes".
	let dir = sandbox("nochange-existing");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut a = args();
	a.model = Some("openrouter:m/first".to_string());
	execute(&a, template_config()).expect("seed a config file");
	execute(&args(), template_config()).expect("no-change run against existing file");

	// Missing file: a default config is created.
	let dir = sandbox("nochange-missing");
	std::env::set_var(DATA_DIR_KEY, &dir);
	execute(&args(), template_config()).expect("no-change run creates default config");
	assert!(
		saved_config_path(&dir).exists(),
		"default config must be created"
	);
}

#[test]
#[serial]
fn test_upgrade_missing_config_errors_and_current_config_succeeds() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let mut a = args();
	a.upgrade = true;

	let dir = sandbox("upgrade-missing");
	std::env::set_var(DATA_DIR_KEY, &dir);
	assert!(
		execute(&a, template_config()).is_err(),
		"missing config file must error"
	);

	let dir = sandbox("upgrade-current");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let path = saved_config_path(&dir);
	std::fs::create_dir_all(path.parent().expect("parent")).expect("config dir");
	std::fs::write(&path, include_str!("../../config-templates/default.toml"))
		.expect("seed current config");
	execute(&a, template_config()).expect("upgrading a current config succeeds");
}
