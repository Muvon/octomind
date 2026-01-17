// Copyright 2025 Muvon Un Limited
//
// OAuth 2.0 Device Flow Implementation (RFC 8628)
// Designed for CLI tools and headless applications

use crate::config::OAuthConfig;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use serde_urlencoded;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

fn now_secs() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or(Duration::from_secs(0))
		.as_secs()
}

/// Device flow response from Step 1
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
	pub device_code: String,
	pub user_code: String,
	pub verification_uri: String,
	pub expires_in: u64,
	pub interval: u64,
}

/// Device flow token response (same as regular token response)
/// Note: GitHub may include additional fields - don't use deny_unknown_fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceTokenResponse {
	pub access_token: String,
	#[serde(default)]
	pub token_type: String,
	#[serde(default)]
	pub scope: String,
	#[serde(default)]
	pub refresh_token: Option<String>,
}

/// Device flow error response (RFC 8628 Section 3.5)
/// GitHub includes extra fields like 'interval' in slow_down responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFlowErrorResponse {
	pub error: String,
	#[serde(default)]
	pub error_description: Option<String>,
	#[serde(default)]
	pub error_uri: Option<String>,
	/// New interval to use after slow_down error (RFC 8628)
	#[serde(default)]
	pub interval: Option<u64>,
}

/// Device code metadata for pending authorization
#[derive(Debug, Clone)]
pub struct PendingDeviceAuth {
	pub device_code: String,
	pub user_code: String,
	pub verification_uri: String,
	pub expires_at: u64, // Unix timestamp
	pub interval: u64,
	pub last_poll: std::time::Instant,
}

// Global cache for pending device authorizations
lazy_static::lazy_static! {
	static ref PENDING_DEVICE_AUTHS: Arc<Mutex<HashMap<String, PendingDeviceAuth>>> =
		Arc::new(Mutex::new(HashMap::new()));
}

/// Start the Device Flow authorization
///
/// Returns the user code and verification URL to show to the user
pub async fn start_device_flow(config: &OAuthConfig) -> Result<DeviceCodeResponse, String> {
	let client = reqwest::Client::new();

	// Build scope string
	let scope = config.scopes.join(" ");

	crate::log_debug!(
		"Starting GitHub Device Flow - client_id: {}, scopes: {}",
		config.client_id,
		scope
	);

	// Step 1: Request device and user verification codes
	let params = [
		("client_id", config.client_id.as_str()),
		("scope", scope.as_str()),
	];

	let form_body =
		serde_urlencoded::to_string(params).map_err(|e| format!("Form error: {}", e))?;

	let response = client
		.post("https://github.com/login/device/code")
		.header(reqwest::header::ACCEPT, "application/json")
		.header(
			reqwest::header::CONTENT_TYPE,
			"application/x-www-form-urlencoded",
		)
		.body(form_body)
		.send()
		.await
		.map_err(|e| format!("Network error: {}", e))?;

	let status = response.status();
	let text = response
		.text()
		.await
		.map_err(|e| format!("Read error: {}", e))?;

	crate::log_debug!("Device code response - status: {}, body: {}", status, text);

	if !status.is_success() {
		// Try to parse error
		if let Ok(flow_err) = serde_json::from_str::<DeviceFlowErrorResponse>(&text) {
			return Err(format!(
				"{} - {}",
				flow_err.error,
				flow_err.error_description.unwrap_or_default()
			));
		}
		return Err(format!("Device code request failed: {} - {}", status, text));
	}

	// Parse successful response
	serde_json::from_str(&text).map_err(|e| format!("Invalid response: {}", e))
}

/// Poll for device flow token
///
/// Call this repeatedly after showing user the code until it returns a token or expires
pub async fn poll_for_device_token(
	config: &OAuthConfig,
	device_code: &str,
) -> Result<DeviceTokenResponse, String> {
	let client = reqwest::Client::new();

	// Step 3: Poll for access token
	let params = [
		("client_id", config.client_id.as_str()),
		("device_code", device_code),
		("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
	];

	let form_body =
		serde_urlencoded::to_string(params).map_err(|e| format!("Form error: {}", e))?;

	let response = client
		.post("https://github.com/login/oauth/access_token")
		.header(reqwest::header::ACCEPT, "application/json")
		.header(
			reqwest::header::CONTENT_TYPE,
			"application/x-www-form-urlencoded",
		)
		.body(form_body)
		.send()
		.await
		.map_err(|e| format!("Network error: {}", e))?;

	let status = response.status();
	let text = response
		.text()
		.await
		.map_err(|e| format!("Read error: {}", e))?;

	crate::log_debug!(
		"Device token poll response - status: {}, body: {}",
		status,
		text
	);

	// RFC 8628: GitHub returns HTTP 200 OK for ALL responses (success and errors)
	// Errors are indicated by the presence of an "error" field in the JSON body
	// MUST check for error field FIRST before trying to parse as success response
	if let Ok(error_response) = serde_json::from_str::<DeviceFlowErrorResponse>(&text) {
		// Return the error code as-is for proper handling in execute_device_flow
		// The interval field (if present) will be used to update polling interval
		return Err(match error_response.error.as_str() {
			"authorization_pending" => "authorization_pending".to_string(),
			"slow_down" => {
				// RFC 8628 Section 3.5: slow_down means increase interval by 5 seconds
				// GitHub may include the new interval value in the response
				if let Some(new_interval) = error_response.interval {
					format!("slow_down:{}", new_interval)
				} else {
					"slow_down".to_string()
				}
			}
			"expired_token" => "expired_token".to_string(),
			"access_denied" => "access_denied".to_string(),
			_ => format!(
				"{} - {}",
				error_response.error,
				error_response.error_description.unwrap_or_default()
			),
		});
	}

	// Check HTTP status only if JSON parsing failed (shouldn't happen with GitHub)
	if !status.is_success() {
		return Err(format!("Token request failed: {} - {}", status, text));
	}

	// Try to parse as successful token response
	match serde_json::from_str::<DeviceTokenResponse>(&text) {
		Ok(token) => Ok(token),
		Err(e) => {
			// GitHub might return URL-encoded response instead of JSON
			crate::log_debug!("Failed to parse as JSON: {}, trying URL-encoded format", e);

			let params: std::collections::HashMap<String, String> =
				serde_urlencoded::from_str(&text).map_err(|parse_err| {
					format!(
						"Invalid response format: JSON error: {}, URL-encoded error: {}",
						e, parse_err
					)
				})?;

			let access_token = params
				.get("access_token")
				.ok_or_else(|| format!("Missing access_token in response: {}", text))?
				.clone();

			let token_type = params
				.get("token_type")
				.unwrap_or(&"bearer".to_string())
				.clone();

			let scope = params.get("scope").unwrap_or(&"".to_string()).clone();

			Ok(DeviceTokenResponse {
				access_token,
				token_type,
				scope,
				refresh_token: params.get("refresh_token").cloned(),
			})
		}
	}
}

/// Get or create pending device authorization for a server
async fn get_or_create_device_auth(
	config: &OAuthConfig,
	server_name: &str,
) -> Result<PendingDeviceAuth, String> {
	let mut auths = PENDING_DEVICE_AUTHS.lock().await;

	// Check if we have a pending auth that's still valid
	if let Some(auth) = auths.get(server_name) {
		if auth.expires_at > now_secs() {
			return Ok(auth.clone());
		} else {
			// Expired, remove it
			auths.remove(server_name);
		}
	}

	// Need to start a new device flow - release lock
	drop(auths);

	let device_response = start_device_flow(config).await?;

	let pending_auth = PendingDeviceAuth {
		device_code: device_response.device_code,
		user_code: device_response.user_code,
		verification_uri: device_response.verification_uri,
		expires_at: now_secs() + device_response.expires_in,
		interval: device_response.interval,
		last_poll: std::time::Instant::now(),
	};

	// Cache it
	let mut auths = PENDING_DEVICE_AUTHS.lock().await;
	auths.insert(server_name.to_string(), pending_auth.clone());

	Ok(pending_auth)
}

/// Execute the complete Device Flow with proper caching
///
/// This function handles the entire flow:
/// 1. Check for existing pending authorization
/// 2. If none, request device/user codes
/// 3. Print instructions to user
/// 4. Poll for token until authorized or expired
pub async fn execute_device_flow(
	config: &OAuthConfig,
	server_name: &str,
) -> Result<String, String> {
	// Get or create pending device authorization
	let pending_auth = get_or_create_device_auth(config, server_name).await?;

	// Show user instructions (only if we just started)
	println!("\n");
	println!("{}", "═".repeat(70));
	// Show user instructions
	println!("\\n");
	println!("{}", "═".repeat(70));
	println!(
		"{}",
		"🔐 GITHUB AUTHORIZATION REQUIRED".bright_cyan().bold()
	);
	println!("{}", "═".repeat(70));
	println!();
	println!(
		"Please visit: {}",
		pending_auth.verification_uri.bright_white()
	);
	println!();
	println!(
		"And enter this code: {}",
		pending_auth.user_code.bright_green().bold()
	);
	println!();
	println!(
		"This code expires in {} minutes.",
		(pending_auth.expires_at - now_secs()) / 60
	);
	println!();
	println!("Waiting for authorization... (press Ctrl+C to cancel)");
	println!("{}", "─".repeat(70));
	println!();

	// Poll for token - RFC 8628 compliant implementation
	let mut interval_seconds = pending_auth.interval;
	let expires_at_timestamp = pending_auth.expires_at; // Unix timestamp
	let mut last_poll_time = std::time::Instant::now();

	println!(
		"🔍 Starting polling loop with interval: {}s",
		interval_seconds
	);

	loop {
		// Check if expired - compare Unix timestamps
		if now_secs() >= expires_at_timestamp {
			// Remove expired auth
			let mut auths = PENDING_DEVICE_AUTHS.lock().await;
			auths.remove(server_name);
			return Err("Authorization timed out. Please try again.".to_string());
		}

		// RFC 8628: Wait at least 'interval' seconds between polls
		let elapsed_since_last_poll = last_poll_time.elapsed();
		if elapsed_since_last_poll < Duration::from_secs(interval_seconds) {
			tokio::time::sleep(Duration::from_secs(interval_seconds) - elapsed_since_last_poll)
				.await;
		}

		// Update last poll time BEFORE making the request
		last_poll_time = std::time::Instant::now();

		// Poll for token
		match poll_for_device_token(config, &pending_auth.device_code).await {
			Ok(token_response) => {
				// Success! Remove pending auth and return token
				println!();
				println!("✅ Authorization successful!");
				println!();

				// Clean up pending auth state
				let mut auths = PENDING_DEVICE_AUTHS.lock().await;
				auths.remove(server_name);

				return Ok(token_response.access_token);
			}
			Err(e) => {
				// Check if error includes new interval (slow_down:N format)
				if e.starts_with("slow_down") {
					// RFC 8628 Section 3.5: slow_down increases interval by 5 seconds
					// GitHub may provide the new interval value
					if let Some(new_interval_str) = e.strip_prefix("slow_down:") {
						if let Ok(new_interval) = new_interval_str.parse::<u64>() {
							interval_seconds = new_interval;
							crate::log_debug!(
								"slow_down: using new interval from GitHub: {}s",
								interval_seconds
							);
						} else {
							interval_seconds += 5;
						}
					} else {
						interval_seconds += 5;
					}
					println!(
						"\nRate limited - slowing down polling (new interval: {}s)...",
						interval_seconds
					);
				} else {
					match e.as_str() {
						"authorization_pending" => {
							// User hasn't authorized yet, keep polling
							print!(".");
							let _ = std::io::Write::flush(&mut std::io::stdout());
						}
						"access_denied" => {
							let mut auths: tokio::sync::MutexGuard<
								'_,
								HashMap<String, PendingDeviceAuth>,
							> = PENDING_DEVICE_AUTHS.lock().await;
							auths.remove(server_name);
							return Err("Authorization was denied. Please try again.".to_string());
						}
						"expired_token" => {
							let mut auths: tokio::sync::MutexGuard<
								'_,
								HashMap<String, PendingDeviceAuth>,
							> = PENDING_DEVICE_AUTHS.lock().await;
							auths.remove(server_name);
							return Err("Authorization code expired. Please try again.".to_string());
						}
						_ => {
							// Some other error - log and continue polling
							crate::log_debug!("Device flow error: {}", e);
							print!(".");
							let _ = std::io::Write::flush(&mut std::io::stdout());
						}
					}
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_device_code_response() {
		let json = r#"{
            "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        }"#;

		let response: DeviceCodeResponse = serde_json::from_str(json).unwrap();
		assert_eq!(
			response.device_code,
			"3584d83530557fdd1f46af8289938c8ef79f9dc5"
		);
		assert_eq!(response.user_code, "WDJB-MJHT");
		assert_eq!(response.verification_uri, "https://github.com/login/device");
		assert_eq!(response.expires_in, 900);
		assert_eq!(response.interval, 5);
	}

	#[test]
	fn test_parse_device_token_response() {
		let json = r#"{
            "access_token": "gho_16C7e42F292c6912E7710c838347Ae178B4a",
            "token_type": "bearer",
            "scope": "repo,gist"
        }"#;

		let response: DeviceTokenResponse = serde_json::from_str(json).unwrap();
		assert_eq!(
			response.access_token,
			"gho_16C7e42F292c6912E7710c838347Ae178B4a"
		);
		assert_eq!(response.token_type, "bearer");
		assert_eq!(response.scope, "repo,gist");
	}

	#[test]
	fn test_parse_device_flow_error() {
		let json = r#"{
            "error": "authorization_pending",
            "error_description": "The authorization request is still pending"
        }"#;

		let error: DeviceFlowErrorResponse = serde_json::from_str(json).unwrap();
		assert_eq!(error.error, "authorization_pending");
		assert_eq!(
			error.error_description,
			Some("The authorization request is still pending".to_string())
		);
	}
}
