// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

use super::super::{LearningConfig, Lesson, McpEndpointConfig};
use super::LearningBackend;
use crate::config::Config;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

pub struct McpBackend {
	store_config: Option<McpEndpointConfig>,
	retrieve_config: Option<McpEndpointConfig>,
}

impl McpBackend {
	pub fn new(config: &LearningConfig) -> Self {
		Self {
			store_config: config.store.clone(),
			retrieve_config: config.retrieve.clone(),
		}
	}

	fn build_store_args(lesson: &Lesson, field_map: &HashMap<String, String>) -> Value {
		let mut args = serde_json::Map::new();
		for (canonical, mcp_field) in field_map {
			if mcp_field.is_empty() {
				continue;
			}
			if let Some(value) = lesson.get_field(canonical) {
				args.insert(mcp_field.clone(), value);
			}
		}
		Value::Object(args)
	}

	fn build_retrieve_args(
		query: &str,
		role: &str,
		project: &str,
		limit: usize,
		field_map: &HashMap<String, String>,
	) -> Value {
		let mut args = serde_json::Map::new();
		let values: HashMap<&str, Value> = [
			("query", Value::String(query.to_string())),
			("role", Value::String(role.to_string())),
			("project", Value::String(project.to_string())),
			("limit", serde_json::json!(limit)),
			("memory_type", serde_json::json!(["learning"])),
		]
		.into();

		for (canonical, mcp_field) in field_map {
			if mcp_field.is_empty() {
				continue;
			}
			if let Some(value) = values.get(canonical.as_str()) {
				args.insert(mcp_field.clone(), value.clone());
			}
		}
		Value::Object(args)
	}

	fn parse_retrieve_result(result: &crate::mcp::McpToolResult) -> Vec<Lesson> {
		let content = result.extract_content();
		if let Ok(lessons) = serde_json::from_str::<Vec<Lesson>>(&content) {
			return lessons;
		}
		if let Ok(obj) = serde_json::from_str::<Value>(&content) {
			for key in ["memories", "results", "data"] {
				if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
					if let Ok(lessons) =
						serde_json::from_value::<Vec<Lesson>>(Value::Array(arr.clone()))
					{
						return lessons;
					}
				}
			}
		}
		content
			.lines()
			.filter(|l| !l.trim().is_empty())
			.map(|l| Lesson {
				content: l.trim().to_string(),
				..Default::default()
			})
			.collect()
	}
}

#[async_trait]
impl LearningBackend for McpBackend {
	async fn store(&self, lesson: &Lesson, config: &Config) -> Result<()> {
		let endpoint = self
			.store_config
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("MCP store not configured in [learning.store]"))?;

		let args = Self::build_store_args(lesson, &endpoint.field_map);
		let call = crate::mcp::McpToolCall {
			tool_name: endpoint.tool.clone(),
			parameters: args,
			tool_id: uuid::Uuid::new_v4().to_string(),
		};

		let (_result, _cost) = crate::mcp::execute_tool_call(&call, config, None).await?;
		Ok(())
	}

	async fn retrieve(
		&self,
		intent: &str,
		patterns: &[String],
		role: &str,
		project: &str,
		limit: usize,
		config: &Config,
	) -> Result<Vec<Lesson>> {
		let endpoint = self
			.retrieve_config
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("MCP retrieve not configured in [learning.retrieve]"))?;

		// Prefer the LLM-prepared natural-language query (patterns[0]); if
		// the LLM call was skipped, fall back to raw user intent so the
		// MCP backend still has something to score against.
		let query = patterns
			.first()
			.cloned()
			.filter(|q| !q.trim().is_empty())
			.unwrap_or_else(|| intent.to_string());
		let args = Self::build_retrieve_args(&query, role, project, limit, &endpoint.field_map);
		let call = crate::mcp::McpToolCall {
			tool_name: endpoint.tool.clone(),
			parameters: args,
			tool_id: uuid::Uuid::new_v4().to_string(),
		};

		let (result, _cost) = crate::mcp::execute_tool_call(&call, config, None).await?;
		Ok(Self::parse_retrieve_result(&result))
	}

	async fn retrieve_all(
		&self,
		role: &str,
		project: &str,
		config: &Config,
	) -> Result<Vec<Lesson>> {
		self.retrieve("", &["*".to_string()], role, project, 100, config)
			.await
	}

	async fn delete(&self, _id: &str, _role: &str, _project: &str, _config: &Config) -> Result<()> {
		anyhow::bail!("deletion is not supported for the MCP backend — manage lessons through the MCP tool directly")
	}
}
