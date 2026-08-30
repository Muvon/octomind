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
fn test_get_octomind_data_dir() {
	let result = get_octomind_data_dir();
	assert!(result.is_ok());

	let path = result.unwrap();
	assert!(path.to_string_lossy().contains("octomind"));

	// The directory should exist after calling the function
	assert!(path.exists());
}

#[test]
fn test_subdirectories() {
	// Test that all subdirectory functions work
	assert!(get_config_dir().is_ok());
	assert!(get_sessions_dir().is_ok());
	assert!(get_logs_dir().is_ok());
	assert!(get_cache_dir().is_ok());
}

#[test]
fn test_run_dir_is_private_and_outside_data_dir() {
	let run = get_run_dir().unwrap();
	assert!(run.exists());
	assert!(!run.starts_with(get_octomind_data_dir().unwrap()));
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let mode = fs::metadata(&run).unwrap().permissions().mode();
		assert_eq!(
			mode & 0o077,
			0,
			"run dir must not be group/world accessible"
		);
	}
}

#[test]
fn test_config_file_path() {
	let config_path = get_config_file_path().unwrap();
	assert!(config_path.to_string_lossy().ends_with("config.toml"));
}
#[cfg(unix)]
#[test]
fn test_session_socket_path_fits_sun_path() {
	// Short names keep their identity.
	let short = session_socket_path("abc").unwrap();
	assert_eq!(short, get_run_dir().unwrap().join("abc.sock"));

	// Long names are shortened to fit the strictest sun_path (104 bytes).
	let long_name = "x".repeat(150);
	let long = session_socket_path(&long_name).unwrap();
	assert!(
		long.as_os_str().len() < SUN_PATH_MAX,
		"path {} exceeds sun_path",
		long.display()
	);
	// Deterministic for the same name, distinct for names sharing the head.
	assert_eq!(long, session_socket_path(&long_name).unwrap());
	assert_ne!(long, session_socket_path(&format!("{long_name}y")).unwrap());
}
