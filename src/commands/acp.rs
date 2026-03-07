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

use clap::Args;

#[derive(Args, Debug)]
pub struct AcpArgs {
	/// Session role: developer (default with layers and tools) or assistant (simple chat without tools)
	#[arg(long, default_value = "developer")]
	pub role: String,
}

/// Execute the acp command — runs Octomind as an ACP agent over stdio

pub async fn execute(args: &AcpArgs, config: &octomind::Config) -> Result<(), anyhow::Error> {
	octomind::acp::run(config.clone(), args.role.clone()).await
}
