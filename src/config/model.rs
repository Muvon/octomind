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
//! supervisor, and compression configuration expose the same fields through
//! [`ModelProfileOverride`]. Tap and workflow mappings remain name-only.

use serde::{Deserialize, Serialize};

use super::ReasoningEffortConfig;
use anyhow::{anyhow, Result};

/// Fully resolved parameters for one model request class.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProfile {
	#[serde(rename = "name")]
	pub model: String,
	pub reasoning_effort: ReasoningEffortConfig,
	pub max_tokens: u32,
	pub temperature: f32,
	pub top_p: f32,
	pub top_k: u32,
	pub max_retries: u32,
	pub retry_timeout: u64,
	pub request_timeout_seconds: u64,
}

impl ModelProfile {
	/// Validate a fully resolved profile. All model-bearing boundaries use this
	/// exact validation path after inheritance has been applied.
	pub fn validate(&self, label: &str) -> Result<()> {
		if self.model.trim().is_empty() {
			return Err(anyhow!("{label}.name cannot be empty"));
		}
		if !(0.0..=2.0).contains(&self.temperature) {
			return Err(anyhow!(
				"{label}.temperature must be between 0.0 and 2.0, got: {}",
				self.temperature
			));
		}
		if !(0.0..=1.0).contains(&self.top_p) {
			return Err(anyhow!(
				"{label}.top_p must be between 0.0 and 1.0, got: {}",
				self.top_p
			));
		}
		if self.top_k > 1000 {
			return Err(anyhow!(
				"{label}.top_k must be between 0 and 1000, got: {}",
				self.top_k
			));
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
	pub temperature: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub top_p: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub top_k: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_retries: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub retry_timeout: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_timeout_seconds: Option<u64>,
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
	temperature: Option<f32>,
	#[serde(default)]
	top_p: Option<f32>,
	#[serde(default)]
	top_k: Option<u32>,
	#[serde(default)]
	max_retries: Option<u32>,
	#[serde(default)]
	retry_timeout: Option<u64>,
	#[serde(default)]
	request_timeout_seconds: Option<u64>,
}

impl<'de> Deserialize<'de> for ModelProfileOverride {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		#[derive(Deserialize)]
		#[serde(untagged)]
		enum Repr {
			LegacyModel(String),
			Fields(ModelProfileOverrideFields),
		}

		Ok(match Repr::deserialize(deserializer)? {
			Repr::LegacyModel(model) => Self {
				model: Some(model),
				..Default::default()
			},
			Repr::Fields(fields) => Self {
				model: fields.model,
				reasoning_effort: fields.reasoning_effort,
				max_tokens: fields.max_tokens,
				temperature: fields.temperature,
				top_p: fields.top_p,
				top_k: fields.top_k,
				max_retries: fields.max_retries,
				retry_timeout: fields.retry_timeout,
				request_timeout_seconds: fields.request_timeout_seconds,
			},
		})
	}
}

impl ModelProfileOverride {
	/// Resolve this override against a complete baseline.
	pub fn resolve(&self, base: &ModelProfile) -> ModelProfile {
		ModelProfile {
			model: self.model.clone().unwrap_or_else(|| base.model.clone()),
			reasoning_effort: self.reasoning_effort.unwrap_or(base.reasoning_effort),
			max_tokens: self.max_tokens.unwrap_or(base.max_tokens),
			temperature: self.temperature.unwrap_or(base.temperature),
			top_p: self.top_p.unwrap_or(base.top_p),
			top_k: self.top_k.unwrap_or(base.top_k),
			max_retries: self.max_retries.unwrap_or(base.max_retries),
			retry_timeout: self.retry_timeout.unwrap_or(base.retry_timeout),
			request_timeout_seconds: self
				.request_timeout_seconds
				.unwrap_or(base.request_timeout_seconds),
		}
	}

	/// Overlay explicitly configured values from `other` onto this override.
	pub fn overlay(&self, other: &Self) -> Self {
		Self {
			model: other.model.clone().or_else(|| self.model.clone()),
			reasoning_effort: other.reasoning_effort.or(self.reasoning_effort),
			max_tokens: other.max_tokens.or(self.max_tokens),
			temperature: other.temperature.or(self.temperature),
			top_p: other.top_p.or(self.top_p),
			top_k: other.top_k.or(self.top_k),
			max_retries: other.max_retries.or(self.max_retries),
			retry_timeout: other.retry_timeout.or(self.retry_timeout),
			request_timeout_seconds: other
				.request_timeout_seconds
				.or(self.request_timeout_seconds),
		}
	}

	/// Validate only explicitly supplied fields. Use [`Self::resolve`] followed
	/// by [`ModelProfile::validate`] whenever a main profile is available.
	pub fn validate_explicit(&self, label: &str) -> Result<()> {
		if self
			.model
			.as_ref()
			.is_some_and(|model| model.trim().is_empty())
		{
			return Err(anyhow!("{label}.name cannot be empty"));
		}
		if let Some(model) = &self.model {
			crate::providers::ProviderFactory::get_provider_for_model(model)
				.map_err(|error| anyhow!("{label}.name '{model}' is invalid: {error}"))?;
		}
		if self
			.temperature
			.is_some_and(|temperature| !(0.0..=2.0).contains(&temperature))
		{
			return Err(anyhow!("{label}.temperature must be between 0.0 and 2.0"));
		}
		if self
			.top_p
			.is_some_and(|top_p| !(0.0..=1.0).contains(&top_p))
		{
			return Err(anyhow!("{label}.top_p must be between 0.0 and 1.0"));
		}
		if self.top_k.is_some_and(|top_k| top_k > 1000) {
			return Err(anyhow!("{label}.top_k must be between 0 and 1000"));
		}
		Ok(())
	}
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
