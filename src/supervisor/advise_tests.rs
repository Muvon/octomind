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

//! Signature, eligibility, arm, note, validation, bookkeeping and record
//! shapes as pure functions; the writer call against the scripted chat stub
//! and the observe-only Noul against the in-process evaluation fake. The data
//! directory, `OLLAMA_API_URL` and the stats sink are process-global, so the
//! tests touching them are serial and assert counter deltas.

use super::*;
use crate::session::chat::test_support::{
	fake_provider_config, final_response, install_fake_evaluation, nouls, spawn_recording_stub, spawn_stub,
	spawn_stub_with_status, DataDirGuard, EnvGuard, FakeEvaluationStep, ENV_LOCK,
};

const RECEIPTS: &str = "#3 [read] view {\"path\":\"src/config.rs\"} → ok (1 KB)\n#4 [mut] shell {\"command\":\"cargo run\"} → ERROR (2 KB)";
const ROLE: &str = "__advise_test_role";
const PROJECT: &str = "__advise_test_proj";

fn call(tool: &str, parameters: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: tool.to_string(),
		parameters,
		tool_id: "t1".to_string(),
	}
}

fn error_result(text: &str) -> McpToolResult {
	McpToolResult::error("shell".to_string(), "t1".to_string(), text.to_string())
}

fn normalized(signature: &StuckSignature) -> &str {
	signature.key.splitn(3, '|').nth(2).expect("key has three parts")
}

fn advice(note: &str) -> serde_json::Value {
	serde_json::json!({"skip": false, "class": "unread_error", "evidence": "#4", "note": note})
}

fn message(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn state() -> AdviseState {
	AdviseState {
		signal: DetectorSignal::Recovery,
		request: "make the loader read settings.toml".to_string(),
		actions: RECEIPTS.to_string(),
		grounds: vec![
			(3, "loading config.toml".to_string()),
			(4, "error: config.toml not found".to_string()),
		],
		..Default::default()
	}
}

fn advise_config() -> crate::config::Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config
}

fn snapshot_u64(key: &str) -> u64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get(key)?.as_u64())
		.unwrap_or(0)
}

fn record_lines() -> Vec<serde_json::Value> {
	let path = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join(RECORD_FILE);
	std::fs::read_to_string(path)
		.expect("record file written")
		.lines()
		.map(|line| serde_json::from_str(line).expect("record line is JSON"))
		.collect()
}

fn keys(value: &serde_json::Value) -> Vec<String> {
	let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
	keys.sort();
	keys
}

#[test]
fn recovery_signature_masks_digits_in_the_first_error_line() {
	let calls = [call("shell", serde_json::json!({"command": "cargo run"}))];
	let results = [
		McpToolResult::success("shell".to_string(), "t0".to_string(), "fine".to_string()),
		error_result("\n\n  Error: Port 8080 in use at line 42  \nsecond line"),
	];
	let signature = stuck_signature(DetectorSignal::Recovery, &calls, &results);
	assert_eq!(signature.key, "recovery|shell|error: port #### in use at line ##");
	assert_eq!(signature.hex.len(), 8);
	assert!(signature.hex.chars().all(|c| c.is_ascii_hexdigit()));

	// The same failure with other numbers is the same stuck state.
	let other_port = [error_result("Error: Port 9090 in use at line 17")];
	assert_eq!(
		stuck_signature(DetectorSignal::Recovery, &calls, &other_port),
		signature
	);
	assert_eq!(
		stuck_signature(DetectorSignal::Recovery, &calls, &results),
		signature,
		"stable"
	);

	let long = [error_result(&"x".repeat(200))];
	assert_eq!(
		normalized(&stuck_signature(DetectorSignal::Recovery, &calls, &long))
			.chars()
			.count(),
		80
	);
	let no_error = [McpToolResult::success(
		"shell".to_string(),
		"t0".to_string(),
		"ok".to_string(),
	)];
	assert_eq!(
		stuck_signature(DetectorSignal::Recovery, &calls, &no_error).key,
		"recovery|shell|"
	);
}

#[test]
fn loop_signature_takes_a_path_then_the_command_then_nothing() {
	let path_call = [call(
		"view",
		serde_json::json!({"path": "src/main.rs", "command": "ignored"}),
	)];
	assert_eq!(
		stuck_signature(DetectorSignal::Loop, &path_call, &[]).key,
		"loop|view|src/main.rs"
	);
	let command_call = [call("shell", serde_json::json!({"command": "cargo test"}))];
	assert_eq!(
		stuck_signature(DetectorSignal::Loop, &command_call, &[]).key,
		"loop|shell|cargo test"
	);
	let bare_call = [call("search", serde_json::json!({"query": "needle"}))];
	assert_eq!(
		stuck_signature(DetectorSignal::Loop, &bare_call, &[]).key,
		"loop|search|"
	);
	assert_eq!(stuck_signature(DetectorSignal::Loop, &[], &[]).key, "loop||");

	let long_path = [call("view", serde_json::json!({"path": "a".repeat(200)}))];
	assert_eq!(
		normalized(&stuck_signature(DetectorSignal::Loop, &long_path, &[]))
			.chars()
			.count(),
		80
	);
	let a = stuck_signature(DetectorSignal::Loop, &path_call, &[]);
	let b = stuck_signature(DetectorSignal::Loop, &command_call, &[]);
	assert_ne!(a.hex, b.hex);
	assert_eq!(a, stuck_signature(DetectorSignal::Loop, &path_call, &[]));
}

#[test]
fn only_loop_and_recovery_past_the_progress_floor_are_eligible() {
	for actions in [0, MIN_ACTIONS - 1, MIN_ACTIONS, MIN_ACTIONS + 10] {
		let past_floor = actions >= MIN_ACTIONS;
		assert_eq!(eligible(DetectorSignal::Loop, actions), past_floor, "loop at {actions}");
		assert_eq!(
			eligible(DetectorSignal::Recovery, actions),
			past_floor,
			"recovery at {actions}"
		);
		assert!(
			!eligible(DetectorSignal::NoProgress, actions),
			"no-progress at {actions}"
		);
		assert!(!eligible(DetectorSignal::None, actions), "none at {actions}");
	}
}

#[test]
fn arm_is_deterministic_and_roughly_one_in_control_every() {
	let sequences = 400u64;
	let controls = (0..sequences)
		.filter(|&sequence| arm("session-a", sequence) == Arm::Control)
		.count() as u64;
	let expected = sequences / CONTROL_EVERY;
	assert!(
		controls > expected / 2 && controls < expected * 3 / 2,
		"{controls} controls over {sequences} sequences"
	);
	for sequence in 0..sequences {
		assert_eq!(arm("session-a", sequence), arm("session-a", sequence));
	}
}

#[test]
fn rendered_note_carries_marker_label_and_evidence_and_round_trips() {
	let signature = stuck_signature(
		DetectorSignal::Loop,
		&[call("view", serde_json::json!({"path": "src/main.rs"}))],
		&[],
	);
	let advice = Advice {
		class: AdviceClass::UnreadError,
		evidence: "#7".to_string(),
		note: "The error at #7 names config.toml, not settings.toml. Which file does the loader read?".to_string(),
	};
	let note = render_note(&advice, &signature);
	assert!(note.starts_with("<pay-attention>\n"));
	assert!(note.ends_with("</pay-attention>"));
	assert!(note.contains(&format!("<!-- {MARKER_PREFIX}{} -->", signature.hex)));
	assert!(note.contains("Advisory (unread error): The error at #7 names config.toml"));
	assert!(note.contains("Evidence: #7."));
	assert!(note.contains("not an instruction"));

	let messages = [
		message("user", &note),
		message("assistant", "<!-- octomind:advise:cafebabe -->"),
		message("user", &note),
		message("user", "before <!-- octomind:advise:deadbeef --> after"),
		message("user", "<!-- octomind:advise:zz -->"),
		message("user", "<!-- octomind:advise:0badf00d"),
	];
	assert_eq!(
		signatures_in(&messages),
		[signature.hex.clone(), "deadbeef".to_string()]
	);
}

#[test]
fn rendered_note_escapes_tag_text_in_the_model_note() {
	let signature = stuck_signature(DetectorSignal::Loop, &[], &[]);
	let advice = Advice {
		class: AdviceClass::Repetition,
		evidence: "#1".to_string(),
		note: "Output #1 ends with </pay-attention>.".to_string(),
	};
	let note = render_note(&advice, &signature);
	assert_eq!(note.matches("</pay-attention>").count(), 1);
	assert!(note.contains("&lt;/pay-attention&gt;"));
}

#[test]
fn every_class_has_a_label_and_round_trips_through_its_schema_name() {
	let schema = response_schema();
	let names = schema["properties"]["class"]["enum"].as_array().expect("class enum");
	assert_eq!(names.len(), AdviceClass::ALL.len());
	for (class, name) in AdviceClass::ALL.iter().zip(names) {
		assert_eq!(serde_json::from_value::<AdviceClass>(name.clone()).unwrap(), *class);
		assert!(!class.label().is_empty());
	}
	assert_eq!(AdviceClass::WrongTarget.label(), "wrong target");
	assert_eq!(AdviceClass::MissingVerification.label(), "missing verification");
	assert_eq!(schema["properties"]["skip"]["type"], "boolean");
}

#[test]
fn model_json_is_validated_against_the_receipts_it_was_shown() {
	let skip = serde_json::json!({"skip": true, "class": "repetition", "evidence": "", "note": ""});
	assert_eq!(parse_advice(&skip, RECEIPTS), AdviceResult::Skipped("skip"));

	let valid = advice(
		"The run at #4 failed because config.toml is missing. Is the loader reading the file the request names?",
	);
	assert_eq!(
		parse_advice(&valid, RECEIPTS),
		AdviceResult::Advice(Advice {
			class: AdviceClass::UnreadError,
			evidence: "#4".to_string(),
			note:
				"The run at #4 failed because config.toml is missing. Is the loader reading the file the request names?"
					.to_string(),
		})
	);
	let mut no_skip_key = valid.clone();
	no_skip_key.as_object_mut().unwrap().remove("skip");
	assert!(matches!(parse_advice(&no_skip_key, RECEIPTS), AdviceResult::Advice(_)));

	for evidence in ["#9", "4", "#", "#4x", ""] {
		let mut cited = valid.clone();
		cited["evidence"] = serde_json::json!(evidence);
		assert_eq!(
			parse_advice(&cited, RECEIPTS),
			AdviceResult::Skipped("evidence"),
			"{evidence:?}"
		);
	}
	// "#3" must not match the prefix of "#34".
	let mut prefix = valid.clone();
	prefix["evidence"] = serde_json::json!("#3");
	assert_eq!(
		parse_advice(&prefix, "#34 [read] view {} → ok (1 KB)"),
		AdviceResult::Skipped("evidence")
	);

	let at_cap = ["word"; ADVICE_MAX_WORDS].join(" ");
	assert!(matches!(
		parse_advice(&advice(&at_cap), RECEIPTS),
		AdviceResult::Advice(_)
	));
	let over_cap = ["word"; ADVICE_MAX_WORDS + 1].join(" ");
	for note in [
		over_cap.as_str(),
		"Look at this:\n```\nfn main() {}\n```",
		"Try this:\n$ cargo test",
		"Indented:\n   $ rm -rf target",
	] {
		assert_eq!(
			parse_advice(&advice(note), RECEIPTS),
			AdviceResult::Skipped("shape"),
			"{note:?}"
		);
	}
	let mut unknown_class = valid.clone();
	unknown_class["class"] = serde_json::json!("vibes");
	assert_eq!(parse_advice(&unknown_class, RECEIPTS), AdviceResult::Skipped("shape"));
	let mut no_note = valid.clone();
	no_note.as_object_mut().unwrap().remove("note");
	assert_eq!(parse_advice(&no_note, RECEIPTS), AdviceResult::Skipped("shape"));
	assert_eq!(parse_advice(&advice("   "), RECEIPTS), AdviceResult::Skipped("shape"));
}

#[test]
fn state_keeps_the_last_receipts_and_cuts_each_output() {
	let actions: String = (1..=12)
		.map(|n| format!("#{n} [read] view {{}} → ok (1 KB)\n\n"))
		.collect();
	let receipts = recent_receipts(&actions);
	let lines: Vec<&str> = receipts.lines().collect();
	assert_eq!(lines.len(), STATE_RECEIPTS);
	assert!(lines[0].starts_with("#5 "));
	assert!(lines[STATE_RECEIPTS - 1].starts_with("#12 "));
	assert_eq!(recent_receipts(RECEIPTS), RECEIPTS);

	let grounds: Vec<(u64, String)> = (1..=10).map(|n| (n, "y".repeat(GROUND_CHARS + 500))).collect();
	let kept = recent_grounds(&grounds);
	assert_eq!(kept.len(), STATE_RECEIPTS);
	assert_eq!(kept[0].0, 3);
	assert!(kept.iter().all(|(_, text)| text.chars().count() == GROUND_CHARS));
	assert_eq!(recent_grounds(&[(1, "short".to_string())]), [(1, "short".to_string())]);
}

#[test]
fn writer_payload_escapes_untrusted_text_and_omits_empty_blocks() {
	let mut full = state();
	full.request = "fix <b>the</b> loader".to_string();
	let outputs = outputs_json(&full.grounds);
	let bare = writer_payload(&full, RECEIPTS, &outputs);
	assert!(bare.contains("<request>\nfix &lt;b&gt;the&lt;/b&gt; loader\n</request>"));
	assert!(bare.contains("<receipts>\n#3 [read]"));
	assert!(bare.contains("<outputs trust=\"untrusted\">"));
	assert!(bare.contains("error: config.toml not found"));
	for absent in [
		"<role>",
		"<plan>",
		"<commands",
		"<mutated_paths>",
		"<prior_advice>",
		"<lessons",
	] {
		assert!(!bare.contains(absent), "{absent}");
	}

	full.role = "Never run the test suite.".to_string();
	full.plan = Some("phase 1: loader".to_string());
	full.recent_commands = vec![("cargo run".to_string(), "error: <eof>".to_string())];
	full.mutated_paths = vec!["src/config.rs".to_string()];
	full.prior_notes = vec!["earlier note".to_string()];
	full.lessons = vec![Lesson {
		content: "the loader reads settings.toml".to_string(),
		created: "2026-11-01T00:00:00Z".to_string(),
		..Default::default()
	}];
	let rich = writer_payload(&full, RECEIPTS, &outputs);
	assert!(rich.contains("<role>\nNever run the test suite.\n</role>"));
	assert!(rich.contains("<plan>\nphase 1: loader\n</plan>"));
	assert!(WRITER_PROMPT.contains("- <role>"));
	assert!(rich.contains("command: cargo run\noutput tail:\nerror: &lt;eof&gt;"));
	assert!(rich.contains("<mutated_paths>\nsrc/config.rs\n</mutated_paths>"));
	assert!(rich.contains("<prior_advice>\n- earlier note\n</prior_advice>"));
	assert!(rich.contains(&format!("<lesson id=\"{}\">", full.lessons[0].file_id())));
	assert!(WRITER_PROMPT.contains("<input_format>"));
}

#[test]
fn open_entries_measure_call_set_change_for_outcome_rounds_only() {
	let signature = stuck_signature(DetectorSignal::Loop, &[], &[]);
	let mut entries = vec![
		open(signature.clone(), Arm::Treated, true, 11),
		open(signature, Arm::Control, false, 22),
	];
	assert!(entries
		.iter()
		.all(|entry| entry.rounds_left == OUTCOME_ROUNDS && !entry.changed));
	for _ in 0..OUTCOME_ROUNDS {
		note_round(&mut entries, 11);
	}
	assert!(!entries[0].changed, "the same call-set is no change");
	assert!(entries[1].changed, "a different call-set is a change");
	assert!(entries.iter().all(|entry| entry.rounds_left == 0));
	note_round(&mut entries, 99);
	assert!(!entries[0].changed, "a closed window records nothing");
	assert_eq!(entries[0].rounds_left, 0);
}

#[test]
fn record_lines_have_the_documented_shape() {
	let signature = stuck_signature(
		DetectorSignal::Recovery,
		&[call("shell", serde_json::json!({}))],
		&[error_result("boom")],
	);
	let lessons = ["lesson-a".to_string()];
	let emitted = emit_line(&EmitRecord {
		session: "s1",
		sequence: 6,
		signal: DetectorSignal::Recovery,
		signature: &signature,
		arm: Arm::Treated,
		result: "advised",
		class: Some(AdviceClass::StaleView),
		evidence: Some("#4"),
		lessons: &lessons,
		p_useful: Some(0.25),
		ms: 120,
	});
	assert_eq!(
		keys(&emitted),
		[
			"arm",
			"class",
			"evidence",
			"lessons",
			"ms",
			"p_useful",
			"result",
			"sequence",
			"session",
			"signal",
			"signature",
			"ts"
		]
	);
	assert_eq!(emitted["signal"], "recovery");
	assert_eq!(emitted["arm"], "treated");
	assert_eq!(emitted["class"], "stale_view");
	assert_eq!(emitted["evidence"], "#4");
	assert_eq!(emitted["lessons"], serde_json::json!(["lesson-a"]));
	assert_eq!(emitted["signature"]["key"], signature.key.as_str());
	assert_eq!(emitted["signature"]["hex"], signature.hex.as_str());

	let control = emit_line(&EmitRecord {
		session: "s1",
		sequence: 7,
		signal: DetectorSignal::Loop,
		signature: &signature,
		arm: Arm::Control,
		result: "control",
		class: None,
		evidence: None,
		lessons: &[],
		p_useful: None,
		ms: 0,
	});
	assert!(control["class"].is_null() && control["evidence"].is_null() && control["p_useful"].is_null());
	assert_eq!(control["arm"], "control");

	let outcome = outcome_line(&open(signature, Arm::Control, false, 5), "s1", "pass");
	assert_eq!(
		keys(&outcome),
		["advised", "arm", "changed_calls", "gate", "session", "signature", "ts"]
	);
	assert_eq!(outcome["gate"], "pass");
	assert_eq!(outcome["changed_calls"], false);
}

#[serial_test::serial]
#[tokio::test]
async fn record_outcome_drains_and_appends_one_line_per_entry() {
	let _guard = ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();
	let signature = stuck_signature(DetectorSignal::Loop, &[], &[]);
	let mut entries = vec![
		open(signature.clone(), Arm::Treated, true, 1),
		open(signature.clone(), Arm::Control, false, 2),
	];
	record_outcome(&mut entries, "s1", "gaps");
	assert!(entries.is_empty());
	record_outcome(&mut entries, "s1", "pass");
	record_emitted(&EmitRecord {
		session: "s1",
		sequence: 4,
		signal: DetectorSignal::Loop,
		signature: &signature,
		arm: Arm::Treated,
		result: "skip",
		class: None,
		evidence: None,
		lessons: &[],
		p_useful: None,
		ms: 3,
	});
	let lines = record_lines();
	assert_eq!(lines.len(), 3, "an empty drain writes nothing");
	assert_eq!(lines[0]["arm"], "treated");
	assert_eq!(lines[1]["arm"], "control");
	assert!(lines[..2].iter().all(|line| line["gate"] == "gaps"));
	assert_eq!(lines[2]["result"], "skip");
}

#[serial_test::serial]
#[tokio::test]
async fn stuck_lessons_keep_tagged_experiences_by_importance() {
	let _guard = ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();
	let signature = StuckSignature {
		key: "loop|view|a".to_string(),
		hex: "abcd1234".to_string(),
	};
	let lesson = |content: &str, memory_type: &str, tag: &str, importance: f64| Lesson {
		content: content.to_string(),
		memory_type: memory_type.to_string(),
		importance,
		tags: vec!["loader".to_string(), tag.to_string()],
		role: ROLE.to_string(),
		project: PROJECT.to_string(),
		created: "2026-11-01T00:00:00Z".to_string(),
		..Default::default()
	};
	let backend = crate::supervisor::learning::backend::FileBackend;
	for record in [
		lesson("low tagged experience", "experience", "stuck:abcd1234", 0.3),
		lesson("top tagged experience", "experience", "stuck:abcd1234", 0.9),
		lesson("mid tagged experience", "experience", "stuck:abcd1234", 0.6),
		lesson("tagged rule", "learning", "stuck:abcd1234", 1.0),
		lesson("other stuck experience", "experience", "stuck:ffff0000", 1.0),
	] {
		backend.store(&record).await.expect("store lesson");
	}
	let found = stuck_lessons(ROLE, PROJECT, &signature).await;
	let contents: Vec<&str> = found.iter().map(|lesson| lesson.content.as_str()).collect();
	assert_eq!(contents, ["top tagged experience", "mid tagged experience"]);
	assert_eq!(found.len(), MAX_STUCK_LESSONS);
	assert!(stuck_lessons(ROLE, "__advise_test_empty", &signature).await.is_empty());
}

#[test]
fn advise_question_is_one_noul_under_its_id() {
	let questions = evaluate::advise_question();
	assert_eq!(
		questions.keys().cloned().collect::<Vec<_>>(),
		[evaluate::ADVISE_QUESTION_ID]
	);
	assert!(matches!(
		questions[evaluate::ADVISE_QUESTION_ID],
		octolib::evaluation::Question::Noul { .. }
	));
	assert_eq!(Seam::Advise.name(), "advise");
	assert_eq!(Seam::ALL[Seam::Advise.index()], Seam::Advise);
}

#[serial_test::serial]
#[tokio::test]
async fn advise_returns_validated_advice_from_the_writer() {
	let _guard = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&["OLLAMA_API_URL"]);
	let note = "The run at #4 failed because config.toml is missing. Is the loader reading the file the request names?";
	let (url, captured) = spawn_recording_stub(vec![final_response(&advice(note).to_string())]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let calls_before = snapshot_u64("advise_calls");
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let advised = advise(&advise_config(), &state(), rx).await;
	assert_eq!(
		advised.result,
		AdviceResult::Advice(Advice {
			class: AdviceClass::UnreadError,
			evidence: "#4".to_string(),
			note: note.to_string(),
		})
	);
	assert_eq!(advised.p_useful, None, "the decision seam is off");
	assert!(snapshot_u64("advise_calls") > calls_before);
	let requests = captured.lock().unwrap().clone();
	assert_eq!(requests.len(), 1);
	let sent = requests[0].to_string();
	assert!(sent.contains("input_format"));
	assert!(sent.contains("<receipts>"));
	assert!(sent.contains("error: config.toml not found"));
}

#[serial_test::serial]
#[tokio::test]
async fn advise_skips_on_a_model_skip_and_on_a_failed_or_unparseable_call() {
	let _guard = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&["OLLAMA_API_URL"]);
	let config = advise_config();

	let skip = serde_json::json!({"skip": true, "class": "repetition", "evidence": "", "note": ""});
	let url = spawn_stub(vec![final_response(&skip.to_string())]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	assert_eq!(
		advise(&config, &state(), rx).await.result,
		AdviceResult::Skipped("skip")
	);

	let url = spawn_stub_with_status(vec![(500, serde_json::json!({"error": "down"}))]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	assert_eq!(
		advise(&config, &state(), rx).await.result,
		AdviceResult::Skipped("unavailable")
	);

	let url = spawn_stub(vec![final_response("sorry, I cannot answer that")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	assert_eq!(
		advise(&config, &state(), rx).await.result,
		AdviceResult::Skipped("unavailable")
	);
}

#[serial_test::serial]
#[tokio::test]
async fn advise_logs_the_observe_only_noul_beside_the_advice() {
	// Holds ENV_LOCK for the whole test, which also covers OLLAMA_API_URL.
	let fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[(
		evaluate::ADVISE_QUESTION_ID,
		0.7,
	)]))])
	.await;
	let _env = EnvGuard::new(&["OLLAMA_API_URL"]);
	let mut config = advise_config();
	config.decision.supervisor = true;
	let skip = serde_json::json!({"skip": true, "class": "repetition", "evidence": "", "note": ""});
	let url = spawn_stub(vec![final_response(&skip.to_string())]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let mut judged = state();
	judged.role = "Never run the test suite.".to_string();
	let advised = advise(&config, &judged, rx).await;
	assert_eq!(
		advised.result,
		AdviceResult::Skipped("skip"),
		"the Noul never skips the writer"
	);
	assert_eq!(advised.p_useful, Some(0.7));
	let requests = fake.requests();
	assert_eq!(requests.len(), 1);
	assert!(requests[0].questions.contains_key(evaluate::ADVISE_QUESTION_ID));
	assert_eq!(requests[0].state["role"], "Never run the test suite.");
	assert_eq!(requests[0].state["receipts"], RECEIPTS);
}
