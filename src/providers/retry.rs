// Copyright 2025 Muvon Un Limited
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

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;

/// Generic retry logic with exponential backoff for providers that don't have smart retry
///
/// This function implements exponential backoff with a configurable base timeout.
/// The delay grows as: base_timeout * 2^attempt, capped at 5 minutes.
///
/// # Arguments
/// * `operation` - The async operation to retry (must return Result<T, E>)
/// * `max_retries` - Maximum number of retry attempts (0 = no retries, just one attempt)
/// * `base_timeout` - Base delay for exponential backoff
/// * `cancellation_token` - Optional token to check for cancellation
///
/// # Returns
/// * `Ok(T)` - Success result from the operation
/// * `Err(E)` - The last error encountered after all retries exhausted
pub async fn retry_with_exponential_backoff<F, T, E>(
	mut operation: F,
	max_retries: u32,
	base_timeout: Duration,
	cancellation_token: Option<&watch::Receiver<bool>>,
) -> Result<T, E>
where
	F: FnMut() -> Pin<Box<dyn Future<Output = Result<T, E>> + Send>>,
	E: std::fmt::Display,
{
	let mut last_error = None;

	for attempt in 0..=max_retries {
		// Check for cancellation before each attempt
		if let Some(token) = cancellation_token {
			if *token.borrow() {
				return Err(last_error.unwrap_or_else(|| {
					// This is a bit tricky since we need to return E, but we know it's cancelled
					// In practice, this shouldn't happen since we check cancellation first
					panic!("Request cancelled before any attempt")
				}));
			}
		}

		match operation().await {
			Ok(result) => return Ok(result),
			Err(e) => {
				crate::log_debug!("🔄 API request attempt {} failed: {}", attempt + 1, e);

				last_error = Some(e);

				// Don't sleep after the last attempt
				if attempt < max_retries {
					// Exponential backoff: base_timeout * 2^attempt
					let delay = base_timeout * 2_u32.pow(attempt);
					// Cap at 5 minutes for safety
					let delay = std::cmp::min(delay, Duration::from_secs(300));

					crate::log_debug!(
						"🔄 Waiting {:?} before retry attempt {}",
						delay,
						attempt + 2
					);

					sleep(delay).await;
				}
			}
		}
	}

	Err(last_error.unwrap())
}

/// Helper to wrap HTTP requests in retry logic
///
/// This is a convenience wrapper that creates the appropriate closure for HTTP requests.
/// It handles cloning of necessary data for each retry attempt.
pub async fn retry_http_request<T, E>(
	max_retries: u32,
	base_timeout: Duration,
	cancellation_token: Option<&watch::Receiver<bool>>,
	request_builder: impl Fn() -> Pin<Box<dyn Future<Output = Result<T, E>> + Send>>,
) -> Result<T, E>
where
	E: std::fmt::Display,
{
	retry_with_exponential_backoff(
		|| request_builder(),
		max_retries,
		base_timeout,
		cancellation_token,
	)
	.await
}
