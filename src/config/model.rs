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

//! One model configuration contract for every model-bearing boundary.
//!
//! The root configuration owns a complete [`ModelProfile`]. Persistent role,
//! and compression configuration expose the same fields through
//! [`ModelProfileOverride`]. Tap and workflow mappings remain name-only.

use serde::{Deserialize, Serialize};

use super::ReasoningEffortConfig;
use anyhow::{anyhow, Result};

/// Request transport for the providers that support a persistent connection
/// (octohub, chatgpt). Other providers ignore it.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequestTransportConfig {
	Http,
	#[default]
	WebSocket,
}

impl RequestTransportConfig {
	pub fn to_octohub(self) -> octolib::OctoHubTransport {
		match self {
			Self::Http => octolib::OctoHubTransport::Http,
			Self::WebSocket => octolib::OctoHubTransport::WebSocket,
		}
	}

	pub fn to_chatgpt(self) -> octolib::ChatGptTransport {
		match self {
			Self::Http => octolib::ChatGptTransport::Http,
			Self::WebSocket => octolib::ChatGptTransport::WebSocket,
		}
	}
}

/// Fully resolved parameters for one model request class.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProfile {
	#[serde(rename = "name")]
	pub model: String,
	pub reasoning_effort: ReasoningEffortConfig,
	pub max_tokens: u32,
	pub max_retries: u32,
	pub retry_timeout: u64,
	pub request_timeout_seconds: u64,
	pub stream_timeout_seconds: u64,
	pub request_transport: RequestTransportConfig,
}

impl ModelProfile {
	/// Validate a fully resolved profile. All model-bearing boundaries use this
	/// exact validation path after inheritance has been applied.
	pub fn validate(&self, label: &str) -> Result<()> {
		if self.model.trim().is_empty() {
			return Err(anyhow!("{label}.name cannot be empty"));
		}
		crate::providers::ProviderFactory::get_provider_for_model(&self.model)
			.map_err(|error| anyhow!("{label}.name '{}' is invalid: {error}", self.model))?;
		Ok(())
	}
}

/// Optional overrides with exactly the same field surface as [`ModelProfile`].
/// Missing fields inherit from the main profile.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ModelProfileOverride {
	#[serde(default, rename = "name", skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reasoning_effort: Option<ReasoningEffortConfig>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_tokens: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_retries: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub retry_timeout: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_timeout_seconds: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stream_timeout_seconds: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_transport: Option<RequestTransportConfig>,
}

#[derive(Deserialize)]
struct ModelProfileOverrideFields {
	#[serde(default, rename = "name")]
	model: Option<String>,
	#[serde(default)]
	reasoning_effort: Option<ReasoningEffortConfig>,
	#[serde(default)]
	max_tokens: Option<u32>,
	#[serde(default)]
	max_retries: Option<u32>,
	#[serde(default)]
	retry_timeout: Option<u64>,
	#[serde(default)]
	request_timeout_seconds: Option<u64>,
	#[serde(default)]
	stream_timeout_seconds: Option<u64>,
	#[serde(default)]
	request_transport: Option<RequestTransportConfig>,
}

impl<'de> Deserialize<'de> for ModelProfileOverride {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		// A hand-written visitor instead of an untagged enum: a bad value inside
		// the table surfaces the real field-level serde error, not an opaque
		// "data did not match any variant".
		struct OverrideVisitor;

		impl<'de> serde::de::Visitor<'de> for OverrideVisitor {
			type Value = ModelProfileOverride;

			fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
				formatter.write_str("a model name string or a model profile table")
			}

			fn visit_str<E: serde::de::Error>(self, model: &str) -> Result<Self::Value, E> {
				Ok(ModelProfileOverride {
					model: Some(model.to_string()),
					..Default::default()
				})
			}

			fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
				let fields =
					ModelProfileOverrideFields::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
				Ok(ModelProfileOverride {
					model: fields.model,
					reasoning_effort: fields.reasoning_effort,
					max_tokens: fields.max_tokens,
					max_retries: fields.max_retries,
					retry_timeout: fields.retry_timeout,
					request_timeout_seconds: fields.request_timeout_seconds,
					stream_timeout_seconds: fields.stream_timeout_seconds,
					request_transport: fields.request_transport,
				})
			}
		}

		deserializer.deserialize_any(OverrideVisitor)
	}
}

impl ModelProfileOverride {
	/// Resolve this override against a complete baseline.
	pub fn resolve(&self, base: &ModelProfile) -> ModelProfile {
		ModelProfile {
			model: self.model.clone().unwrap_or_else(|| base.model.clone()),
			reasoning_effort: self.reasoning_effort.unwrap_or(base.reasoning_effort),
			max_tokens: self.max_tokens.unwrap_or(base.max_tokens),
			max_retries: self.max_retries.unwrap_or(base.max_retries),
			retry_timeout: self.retry_timeout.unwrap_or(base.retry_timeout),
			request_timeout_seconds: self.request_timeout_seconds.unwrap_or(base.request_timeout_seconds),
			stream_timeout_seconds: self.stream_timeout_seconds.unwrap_or(base.stream_timeout_seconds),
			request_transport: self.request_transport.unwrap_or(base.request_transport),
		}
	}

	/// Overlay explicitly configured values from `other` onto this override.
	pub fn overlay(&self, other: &Self) -> Self {
		Self {
			model: other.model.clone().or_else(|| self.model.clone()),
			reasoning_effort: other.reasoning_effort.or(self.reasoning_effort),
			max_tokens: other.max_tokens.or(self.max_tokens),
			max_retries: other.max_retries.or(self.max_retries),
			retry_timeout: other.retry_timeout.or(self.retry_timeout),
			request_timeout_seconds: other.request_timeout_seconds.or(self.request_timeout_seconds),
			stream_timeout_seconds: other.stream_timeout_seconds.or(self.stream_timeout_seconds),
			request_transport: other.request_transport.or(self.request_transport),
		}
	}

	/// Validate only explicitly supplied fields. Use [`Self::resolve`] followed
	/// by [`ModelProfile::validate`] whenever a main profile is available.
	pub fn validate_explicit(&self, label: &str) -> Result<()> {
		if self.model.as_ref().is_some_and(|model| model.trim().is_empty()) {
			return Err(anyhow!("{label}.name cannot be empty"));
		}
		if let Some(model) = &self.model {
			crate::providers::ProviderFactory::get_provider_for_model(model)
				.map_err(|error| anyhow!("{label}.name '{model}' is invalid: {error}"))?;
		}
		Ok(())
	}
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
