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

//! The catalog the code-mode tests share.

use super::catalog::Catalog;
use serde_json::json;

/// Two servers: `files` (plain schemas) and `my-search` (a name that is not a
/// JavaScript identifier, an enum, and a `$ref` to a nested object).
pub fn catalog() -> Catalog {
	let mut catalog = Catalog::default();
	catalog.add(
		"files",
		"read",
		"Read a file.\nReturns its text.",
		json!({
			"type": "object",
			"properties": {
				"limit": { "type": "integer", "description": "Lines to return" },
				"path": { "type": "string", "description": "File to read" }
			},
			"required": ["path"],
			"additionalProperties": false
		}),
	);
	catalog.add(
		"files",
		"write",
		"Write a file.",
		json!({
			"type": "object",
			"properties": {
				"content": { "type": "string" },
				"path": { "type": "string" }
			},
			"required": ["content", "path"]
		}),
	);
	catalog.add(
		"my-search",
		"find-all",
		"Search the index.",
		json!({
			"type": "object",
			"properties": {
				"mode": { "enum": ["code", "docs"] },
				"page": { "$ref": "#/$defs/Page" },
				"query": { "type": "string" }
			},
			"required": ["query"],
			"$defs": {
				"Page": {
					"type": "object",
					"properties": {
						"after": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
						"size": { "type": "integer", "description": "Hits per page" }
					},
					"required": ["size"]
				}
			}
		}),
	);
	catalog
}
