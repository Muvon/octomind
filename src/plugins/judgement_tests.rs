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

//! What each judgement asks a plugin and how an answer is read. The plugin is
//! a stub; every test uses a session id of its own because the host is shared.

use super::*;
use crate::plugins::test_support::{connect_stub, StubPlugin};
use serde_json::json;
use std::path::Path;

fn choice_answer(id: &str, choice: &str, probabilities: &[(&str, f64)]) -> Value {
	let probabilities: BTreeMap<&str, f64> = probabilities.iter().copied().collect();
	json!({"judgements": {id: {"type": "choice", "choice": choice, "probabilities": probabilities}}})
}

/// A stub that answers every `judge` event with `answer`, switched on for `session_id`.
async fn judging_stub(session_id: &str, answer: Value) -> StubPlugin {
	let stub = StubPlugin::new(&["judge"]).answer(answer);
	connect_stub(session_id, "developer", Path::new("/tmp"), &stub).await;
	stub
}

const ENTRIES: [(&str, &str); 2] = [("a", "about a"), ("b", "about b")];

#[tokio::test]
async fn each_roster_sends_its_purpose_its_question_and_the_none_option() {
	let rosters = [
		(&SKILLS, "skills", "skill"),
		(&CAPABILITIES, "capabilities", "capability"),
		(&TAPS, "taps", "specialist"),
	];
	for (roster, purpose, question_id) in rosters {
		let session_id = format!("judgement-payload-{purpose}");
		let stub = judging_stub(&session_id, choice_answer(question_id, NONE, &[(NONE, 1.0)])).await;

		roster.pick(&session_id, "the request", ENTRIES).await;

		let asked = stub.sent("judge");
		assert_eq!(asked.len(), 1, "{purpose}");
		assert_eq!(asked[0]["purpose"], purpose);
		assert_eq!(asked[0]["subject"], "the request");
		let question = &asked[0]["questions"][question_id];
		assert_eq!(question["type"], "choice", "{purpose}");
		assert!(!question["text"].as_str().unwrap().is_empty(), "{purpose}");
		assert_eq!(
			question["options"],
			json!({"a": "about a", "b": "about b", NONE: roster.none}),
			"{purpose}"
		);
		assert!(asked[0].get("items").is_none(), "{purpose}");
		crate::plugins::end_session(&session_id);
	}
}

#[tokio::test]
async fn the_pick_carries_the_choice_and_its_probabilities() {
	let session_id = "judgement-pick";
	judging_stub(session_id, choice_answer("skill", "b", &[("a", 0.25), ("b", 0.75)])).await;

	let pick = SKILLS.pick(session_id, "the request", ENTRIES).await.expect("a pick");

	assert_eq!(pick.choice, "b");
	assert_eq!(pick.probability("b"), 0.75);
	assert_eq!(pick.probability("a"), 0.25);
	assert_eq!(pick.probability("c"), 0.0);
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn an_answer_naming_an_option_that_was_not_offered_gives_none() {
	let session_id = "judgement-unknown-option";
	judging_stub(session_id, choice_answer("skill", "zzz", &[("zzz", 1.0)])).await;

	assert!(SKILLS.pick(session_id, "the request", ENTRIES).await.is_none());
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn an_answer_with_no_judgements_gives_none() {
	let session_id = "judgement-empty-answer";
	judging_stub(session_id, json!({})).await;

	assert!(SKILLS.pick(session_id, "the request", ENTRIES).await.is_none());
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn a_judgement_for_another_question_gives_none() {
	let session_id = "judgement-other-question";
	judging_stub(session_id, choice_answer("capability", "a", &[("a", 1.0)])).await;

	assert!(SKILLS.pick(session_id, "the request", ENTRIES).await.is_none());
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn with_no_plugin_that_judges_nothing_is_asked() {
	assert!(SKILLS
		.pick("judgement-no-plugin", "the request", ENTRIES)
		.await
		.is_none());
	assert!(still_needed("judgement-no-plugin", json!({}), Vec::new())
		.await
		.is_none());

	let session_id = "judgement-not-subscribed";
	let stub = StubPlugin::new(&["tool.after"]).answer(choice_answer("skill", "a", &[("a", 1.0)]));
	connect_stub(session_id, "developer", Path::new("/tmp"), &stub).await;

	assert!(SKILLS.pick(session_id, "the request", ENTRIES).await.is_none());
	assert!(stub.sent("judge").is_empty());
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn an_applied_pick_reaches_the_plugin_with_its_next_event() {
	let session_id = "judgement-applied";
	let stub = judging_stub(session_id, choice_answer("skill", "a", &[("a", 0.9)])).await;

	let pick = SKILLS.pick(session_id, "first", ENTRIES).await.expect("a pick");
	pick.applied(session_id);
	SKILLS.pick(session_id, "second", ENTRIES).await.expect("a pick");
	SKILLS.pick(session_id, "third", ENTRIES).await.expect("a pick");

	let asked = stub.sent("judge");
	assert!(asked[0].get("judged").is_none());
	assert_eq!(asked[1]["judged"], json!({"skills": 1}));
	assert!(asked[2].get("judged").is_none(), "an event reports it once");
	crate::plugins::end_session(session_id);
}

fn unit(id: &str) -> JudgeItem {
	JudgeItem {
		id: id.to_string(),
		data: json!({"id": id}),
	}
}

fn yes_no_answer(answers: &[(&str, f64)]) -> Value {
	let judgements: BTreeMap<&str, Value> = answers
		.iter()
		.map(|(id, yes)| (*id, json!({"type": "yes_no", "yes": yes})))
		.collect();
	json!({ "judgements": judgements })
}

#[tokio::test]
async fn the_fold_asks_one_yes_or_no_question_over_the_units() {
	let session_id = "judgement-fold";
	let stub = judging_stub(session_id, yes_no_answer(&[("p1", 0.2)])).await;

	let needed = still_needed(session_id, json!({"task": "ship it"}), vec![unit("p1"), unit("p2")])
		.await
		.expect("an answer");

	let asked = stub.sent("judge");
	assert_eq!(asked.len(), 1);
	assert_eq!(asked[0]["purpose"], COMPRESSION);
	assert_eq!(asked[0]["subject"], json!({"task": "ship it"}));
	assert!(asked[0].get("questions").is_none());
	assert_eq!(asked[0]["items"]["key"], "units");
	let question = asked[0]["items"]["question"].as_str().unwrap();
	assert!(question.contains("{slot}") && question.contains("{id}"), "{question}");
	assert_eq!(
		asked[0]["items"]["items"],
		json!([{"id": "p1", "data": {"id": "p1"}}, {"id": "p2", "data": {"id": "p2"}}])
	);
	assert_eq!(needed.probability("p1"), Some(0.2));
	assert_eq!(
		needed.probability("p2"),
		None,
		"a unit the plugin left out has no answer"
	);
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn a_choice_answer_for_a_unit_is_not_a_probability() {
	let session_id = "judgement-fold-choice";
	judging_stub(session_id, choice_answer("p1", "a", &[("a", 1.0)])).await;

	let needed = still_needed(session_id, json!({}), vec![unit("p1")])
		.await
		.expect("an answer");

	assert_eq!(needed.probability("p1"), None);
	crate::plugins::end_session(session_id);
}

#[tokio::test]
async fn demoted_units_reach_the_plugin_with_its_next_event() {
	let session_id = "judgement-fold-applied";
	let stub = judging_stub(session_id, yes_no_answer(&[("p1", 0.1)])).await;

	let needed = still_needed(session_id, json!({}), vec![unit("p1")])
		.await
		.expect("an answer");
	needed.applied(session_id, 3);
	still_needed(session_id, json!({}), vec![unit("p1")])
		.await
		.expect("an answer");

	assert_eq!(stub.sent("judge")[1]["judged"], json!({"compression": 3}));
	crate::plugins::end_session(session_id);
}
