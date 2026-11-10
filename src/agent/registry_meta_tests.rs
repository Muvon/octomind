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
fn parses_title_and_description_from_header_comments() {
	let raw = "# agents/developer/general.toml\n\
		           # Agent: developer:general\n\
		           # Title: General Developer\n\
		           # Description: Elite senior developer.\n\
		           \n\
		           [[roles]]\n\
		           temperature = 0.1\n";
	let m = parse_agent_meta(raw, "developer:general").unwrap();
	assert_eq!(m.title, "General Developer");
	assert_eq!(m.description, "Elite senior developer.");
}

#[test]
fn errors_when_title_missing() {
	let raw = "# Description: Only description\n[[roles]]\n";
	let err = parse_agent_meta(raw, "x:y").unwrap_err().to_string();
	assert!(err.contains("Title"));
}

#[test]
fn errors_when_description_missing() {
	let raw = "# Title: Only title\n[[roles]]\n";
	let err = parse_agent_meta(raw, "x:y").unwrap_err().to_string();
	assert!(err.contains("Description"));
}

#[test]
fn stops_at_first_non_comment_line() {
	// A `# Title:` after the header block should NOT be picked up.
	let raw = "# Title: Real\n\
		           # Description: Real desc\n\
		           [[roles]]\n\
		           system = \"# Title: not metadata\"\n";
	let m = parse_agent_meta(raw, "x:y").unwrap();
	assert_eq!(m.title, "Real");
	assert_eq!(m.description, "Real desc");
}

#[test]
fn header_reads_title_and_description() {
	let raw = "# Agent: developer:chief\n\
		           # Title: Engineering Orchestrator\n\
		           # Description: Controls delivery.\n\
		           [[roles]]\n";
	let header = parse_agent_header(raw);
	assert_eq!(header.title.as_deref(), Some("Engineering Orchestrator"));
	assert_eq!(header.description.as_deref(), Some("Controls delivery."));
}

#[test]
fn header_with_missing_fields_leaves_them_empty() {
	let header = parse_agent_header("# Agent: x:y\n[[roles]]\n");
	assert!(header.title.is_none());
	assert!(header.description.is_none());

	let empty_title = parse_agent_header("# Title:   \n# Description: Only this\n");
	assert!(empty_title.title.is_none(), "an empty value counts as missing");
	assert_eq!(empty_title.description.as_deref(), Some("Only this"));
}
