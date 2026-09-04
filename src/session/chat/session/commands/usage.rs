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

//! /usage — spend and quotas for the signed-in Octomind account.
//!
//! This is the ACCOUNT, not this session: `/info` covers the session's own tokens
//! and cost. The account spans the hub and cloud machines, which is why one set of
//! caps covers both. Not signed in is a normal state — the CLI works fine against
//! your own provider keys — so it says so and stops, rather than erroring.

use super::{CommandOutput, CommandResult};
use crate::account;
use anyhow::Result;

pub async fn handle_usage() -> Result<CommandResult> {
	let usage = match account::usage().await {
		Ok(Some(u)) => u,
		Ok(None) => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Usage {
					signed_in: false,
					account: None,
					windows: vec![],
					always_free_models: vec![],
					balance_usd: 0.0,
					storage_gb: 0.0,
					storage_quota_gb: 0.0,
					network_used_gb: 0.0,
					network_included_gb: 0.0,
				},
			)));
		}
		Err(e) => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: format!("Could not read account usage: {e}"),
					context: Some(serde_json::json!({
						"hint": "Run `octomind login` to sign in, or set OCTOMIND_API_URL if you're testing against a local API."
					})),
				},
			)));
		}
	};

	// Best-effort: the numbers are the point, the email is a nicety.
	let account = account::whoami()
		.await
		.ok()
		.flatten()
		.map(|a| format!("{} ({})", a.email, a.plan));

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Usage {
			signed_in: true,
			account,
			// ONE window (spec/pricing-v2.md §1.1) — its label comes from the
			// server, so the CLI never has to know whether this account is on a
			// billing period or a free 7-day slice. `month` is the pre-v2 shape,
			// read only so /usage still works against an un-upgraded control
			// plane; the CLI ships ahead of the API routinely.
			windows: usage
				.window
				.as_ref()
				.or(usage.month.as_ref())
				.map(|w| vec![window(w.label.as_deref().unwrap_or("this period"), w)])
				.unwrap_or_default(),
			always_free_models: usage.always_free_models.clone(),
			balance_usd: usage.balance_usd,
			storage_gb: usage.storage_gb,
			storage_quota_gb: usage.storage_quota_gb,
			network_used_gb: usage.network.used_gb,
			network_included_gb: usage.network.included_gb,
		},
	)))
}

fn window(label: &str, w: &account::Window) -> UsageWindow {
	UsageWindow {
		label: label.to_string(),
		spent_usd: w.spent_usd,
		reserved_usd: w.reserved_usd,
		allowance_usd: w.allowance_usd,
		resets_at: w.resets_at.clone(),
	}
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageWindow {
	pub label: String,
	pub spent_usd: f64,
	/// Committed by running machines until the reset — None on older servers.
	pub reserved_usd: Option<f64>,
	pub allowance_usd: f64,
	pub resets_at: String,
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
