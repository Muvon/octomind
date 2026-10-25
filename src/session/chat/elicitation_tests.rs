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
use serde_json::json;

fn schema(properties: Value) -> ElicitationSchema {
	serde_json::from_value(json!({ "type": "object", "properties": properties })).expect("valid schema fixture")
}

fn choice(value: &str, title: &str) -> Choice {
	Choice {
		value: value.to_string(),
		title: title.to_string(),
	}
}

fn colors() -> Vec<Choice> {
	vec![choice("red", "red"), choice("green", "green"), choice("blue", "blue")]
}

#[test]
fn fields_follow_map_order_without_property_order() {
	let mut parsed = schema(json!({
		"b": { "type": "string" },
		"a": { "type": "string" }
	}));
	parsed.property_order = None;
	let keys: Vec<_> = fields(&parsed).unwrap().into_iter().map(|f| f.key).collect();
	assert_eq!(keys, ["a", "b"]);
}

#[test]
fn fields_follow_property_order_then_remaining_keys() {
	let mut parsed = schema(json!({
		"a": { "type": "string" },
		"b": { "type": "string" },
		"c": { "type": "string" }
	}));
	parsed.property_order = Some(vec!["c".into(), "missing".into(), "a".into(), "c".into()]);
	let keys: Vec<_> = fields(&parsed).unwrap().into_iter().map(|f| f.key).collect();
	assert_eq!(keys, ["c", "a", "b"]);
}

#[test]
fn fields_carry_required_default_title_and_description() {
	let mut parsed = schema(json!({
		"name": { "type": "string", "title": "Full name", "description": "As on your passport", "default": "Ann" },
		"age": { "type": "integer" }
	}));
	parsed.required = Some(vec!["name".into()]);
	let list = fields(&parsed).unwrap();
	let name = list.iter().find(|f| f.key == "name").unwrap();
	assert!(name.required);
	assert_eq!(name.label, "Full name");
	assert_eq!(name.description.as_deref(), Some("As on your passport"));
	assert_eq!(name.default, Some(json!("Ann")));
	let age = list.iter().find(|f| f.key == "age").unwrap();
	assert!(!age.required);
	assert_eq!(age.label, "age");
	assert_eq!(age.default, None);
}

#[test]
fn unsupported_property_type_names_the_property() {
	let parsed = schema(json!({ "ok": { "type": "string" } }));
	let error = field_from_schema("tags", &json!({ "type": "object" }), false).unwrap_err();
	assert!(error.contains("tags"));
	assert!(fields(&parsed).is_ok());
	assert!(field_from_schema(
		"list",
		&json!({ "type": "array", "items": { "type": "string" } }),
		false
	)
	.is_err());
}

#[test]
fn schema_kinds_are_detected() {
	let parsed = schema(json!({
		"flag": { "type": "boolean" },
		"count": { "type": "integer", "minimum": 1, "maximum": 5 },
		"ratio": { "type": "number", "minimum": 0.5 },
		"mail": { "type": "string", "format": "email", "minLength": 3, "maxLength": 40 },
		"pick": { "type": "string", "oneOf": [{ "const": "a", "title": "Alpha" }, { "const": "b", "title": "Beta" }] },
		"many": { "type": "array", "minItems": 1, "maxItems": 2, "items": { "type": "string", "enum": ["x", "y"] } }
	}));
	let list = fields(&parsed).unwrap();
	let kind = |key: &str| list.iter().find(|f| f.key == key).unwrap().kind.clone();
	assert_eq!(kind("flag"), Kind::Boolean);
	assert_eq!(
		kind("count"),
		Kind::Integer {
			min: Some(1),
			max: Some(5)
		}
	);
	assert_eq!(
		kind("ratio"),
		Kind::Number {
			min: Some(0.5),
			max: None
		}
	);
	assert_eq!(
		kind("mail"),
		Kind::Text {
			min_len: Some(3),
			max_len: Some(40),
			format: TextFormat::Email
		}
	);
	assert_eq!(
		kind("pick"),
		Kind::Choice(vec![choice("a", "Alpha"), choice("b", "Beta")])
	);
	assert_eq!(
		kind("many"),
		Kind::MultiChoice {
			options: vec![choice("x", "x"), choice("y", "y")],
			min_items: Some(1),
			max_items: Some(2)
		}
	);
}

#[test]
fn enum_names_become_titles() {
	let definition = json!({ "type": "string", "enum": ["a", "b"], "enumNames": ["Alpha", "Beta"] });
	let field = field_from_schema("pick", &definition, false).unwrap();
	assert_eq!(
		field.kind,
		Kind::Choice(vec![choice("a", "Alpha"), choice("b", "Beta")])
	);
}

#[test]
fn boolean_answers() {
	for yes in ["y", "YES", "True"] {
		assert_eq!(parse_answer(&Kind::Boolean, yes), Ok(json!(true)));
	}
	for no in ["n", "No", "FALSE"] {
		assert_eq!(parse_answer(&Kind::Boolean, no), Ok(json!(false)));
	}
	assert!(parse_answer(&Kind::Boolean, "maybe").is_err());
}

#[test]
fn integer_bounds() {
	let kind = Kind::Integer {
		min: Some(1),
		max: Some(5),
	};
	assert_eq!(parse_answer(&kind, " 3 "), Ok(json!(3)));
	assert!(parse_answer(&kind, "0").is_err());
	assert!(parse_answer(&kind, "6").is_err());
	assert!(parse_answer(&kind, "2.5").is_err());
	assert!(parse_answer(&kind, "x").is_err());
}

#[test]
fn number_bounds_and_finiteness() {
	let kind = Kind::Number {
		min: Some(0.0),
		max: Some(1.0),
	};
	assert_eq!(parse_answer(&kind, "0.25"), Ok(json!(0.25)));
	assert!(parse_answer(&kind, "-0.1").is_err());
	assert!(parse_answer(&kind, "1.1").is_err());
	let open = Kind::Number { min: None, max: None };
	assert!(parse_answer(&open, "inf").is_err());
	assert!(parse_answer(&open, "NaN").is_err());
}

fn text(min_len: Option<usize>, max_len: Option<usize>, format: TextFormat) -> Kind {
	Kind::Text {
		min_len,
		max_len,
		format,
	}
}

#[test]
fn text_length_counts_chars() {
	let kind = text(Some(2), Some(3), TextFormat::Plain);
	assert_eq!(parse_answer(&kind, "äö"), Ok(json!("äö")));
	assert!(parse_answer(&kind, "a").is_err());
	assert!(parse_answer(&kind, "abcd").is_err());
}

#[test]
fn text_formats() {
	let email = text(None, None, TextFormat::Email);
	assert!(parse_answer(&email, "a@b.co").is_ok());
	for bad in ["ab", "@b.co", "a@", "a b@c.co"] {
		assert!(parse_answer(&email, bad).is_err(), "{bad}");
	}

	let uri = text(None, None, TextFormat::Uri);
	assert!(parse_answer(&uri, "https://example.com/x").is_ok());
	assert!(parse_answer(&uri, "not a uri").is_err());

	let date = text(None, None, TextFormat::Date);
	assert!(parse_answer(&date, "2026-02-28").is_ok());
	assert!(parse_answer(&date, "2026-02-30").is_err());
	assert!(parse_answer(&date, "28/02/2026").is_err());

	let date_time = text(None, None, TextFormat::DateTime);
	assert!(parse_answer(&date_time, "2026-01-31T09:30:00Z").is_ok());
	assert!(parse_answer(&date_time, "2026-01-31 09:30").is_err());

	let unknown = field_from_schema("x", &json!({ "type": "string", "format": "hostname" }), false).unwrap();
	assert_eq!(unknown.kind, text(None, None, TextFormat::Plain));
}

#[test]
fn choice_by_number_or_value() {
	let kind = Kind::Choice(colors());
	assert_eq!(parse_answer(&kind, "2"), Ok(json!("green")));
	assert_eq!(parse_answer(&kind, "blue"), Ok(json!("blue")));
	assert!(parse_answer(&kind, "0").is_err());
	assert!(parse_answer(&kind, "4").is_err());
	assert!(parse_answer(&kind, "purple").is_err());
}

#[test]
fn choice_returns_const_not_title() {
	let kind = Kind::Choice(vec![choice("a", "Alpha")]);
	assert_eq!(parse_answer(&kind, "1"), Ok(json!("a")));
	assert!(parse_answer(&kind, "Alpha").is_err());
}

#[test]
fn multi_choice_selection_and_limits() {
	let kind = Kind::MultiChoice {
		options: colors(),
		min_items: Some(1),
		max_items: Some(2),
	};
	assert_eq!(parse_answer(&kind, "1, blue"), Ok(json!(["red", "blue"])));
	assert_eq!(parse_answer(&kind, "2,2"), Ok(json!(["green"])));
	assert!(parse_answer(&kind, ",").is_err());
	assert!(parse_answer(&kind, "1,2,3").is_err());
	assert!(parse_answer(&kind, "1,purple").is_err());
	assert!(parse_answer(&kind, "9").is_err());
}

#[test]
fn unsupported_mode_url_without_host_is_invalid_params() {
	let request = ElicitRequestParams::UrlElicitationParams {
		meta: None,
		message: "m".into(),
		url: "data:text/plain,hi".into(),
		elicitation_id: "id".into(),
	};
	assert!(Prompt::plan(request).is_err());
}
