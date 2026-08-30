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
fn parse_simple_header() {
	let h = "@description Does the thing.\n@param *target string Path to file\n@param force boolean Overwrite\n";
	let meta = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
	assert_eq!(meta.description, "Does the thing.");
	assert_eq!(meta.params.len(), 2);
	assert_eq!(meta.params[0].name, "target");
	assert_eq!(meta.params[0].ty, "string");
	assert!(meta.params[0].required);
	assert_eq!(meta.params[1].name, "force");
	assert_eq!(meta.params[1].ty, "boolean");
	assert!(!meta.params[1].required);
}

#[test]
fn star_prefix_marks_required() {
	// no star → optional (default)
	let h = "@description x\n@param a string the a\n";
	let m = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
	assert!(!m.params[0].required);

	// star → required, name has no star
	let h = "@description x\n@param *a string the a\n";
	let m = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
	assert!(m.params[0].required);
	assert_eq!(m.params[0].name, "a");
}

#[test]
fn lone_star_param_is_skipped() {
	let h = "@description x\n@param * string oops\n@param *real string ok\n";
	let m = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
	assert_eq!(m.params.len(), 1);
	assert_eq!(m.params[0].name, "real");
}

#[test]
fn multiline_description_continues() {
	let h = "@description Line one.\nLine two.\n@param x string The x\n";
	let meta = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
	assert_eq!(meta.description, "Line one.\nLine two.");
	assert_eq!(meta.params.len(), 1);
}

#[test]
fn header_extraction_skips_shebang_and_stops_at_code() {
	let src = "#!/usr/bin/env bash\n# @description Hi.\n# @param a string the a\necho hello\n# not part of header\n";
	let h = extract_header(src);
	assert!(h.contains("@description Hi."));
	assert!(h.contains("@param a"));
	assert!(!h.contains("not part of header"));
}

#[test]
fn slash_slash_comments_work() {
	let src = "#!/usr/bin/env node\n// @description JS tool\n// @param msg string The message\nconsole.log('hi')\n";
	let h = extract_header(src);
	let meta = parse_header(&h, Path::new("/tmp/x"), "x").unwrap();
	assert_eq!(meta.description, "JS tool");
	assert_eq!(meta.params[0].name, "msg");
}

#[test]
fn missing_description_errors() {
	let h = "@param x string just a thing\n";
	assert!(parse_header(h, Path::new("/tmp/x"), "x").is_err());
}

#[test]
fn invalid_names_rejected() {
	assert!(!is_valid_tool_name(""));
	assert!(!is_valid_tool_name(".hidden"));
	assert!(!is_valid_tool_name("-leading-dash"));
	assert!(!is_valid_tool_name("has space"));
	assert!(!is_valid_tool_name("dot.ext"));
	assert!(is_valid_tool_name("toola"));
	assert!(is_valid_tool_name("tool_b"));
	assert!(is_valid_tool_name("tool-3"));
}

#[test]
fn type_aliases_normalize() {
	assert_eq!(normalize_type("str"), "string");
	assert_eq!(normalize_type("INT"), "integer");
	assert_eq!(normalize_type("bool"), "boolean");
	assert_eq!(normalize_type("unknown"), "string");
}

#[test]
fn to_function_builds_schema() {
	let meta = LocalToolMeta {
		name: "doit".into(),
		description: "Do it.".into(),
		params: vec![
			ParamDef {
				name: "a".into(),
				ty: "string".into(),
				description: "the a".into(),
				required: true,
			},
			ParamDef {
				name: "b".into(),
				ty: "integer".into(),
				description: "the b".into(),
				required: false,
			},
		],
		path: PathBuf::from("/tmp/doit"),
	};
	let f = meta.to_function();
	assert_eq!(f.name, "doit");
	let req = f.parameters["required"].as_array().unwrap();
	assert_eq!(req.len(), 1);
	assert_eq!(req[0], "a");
	assert_eq!(f.parameters["properties"]["a"]["type"], "string");
	assert_eq!(f.parameters["properties"]["b"]["type"], "integer");
}
