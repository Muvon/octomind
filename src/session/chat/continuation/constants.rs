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

// Session continuation constants

// All constants kept internal - no configuration needed
pub const SUMMARY_REQUEST_PROMPT: &str = r#"
CRITICAL: Session approaching token limits. Provide COMPREHENSIVE handoff summary to continue work seamlessly from scratch:

## MAIN OBJECTIVE & SCOPE
What we're building/fixing/implementing:
- Primary goal and why it matters
- Scope boundaries and what's included/excluded
- Success criteria and expected outcomes

## DETAILED PROGRESS ACCOMPLISHED
Complete breakdown of what's been done:
- Specific code changes made (functions, files, logic)
- Configuration changes and settings modified
- Tools executed and their results/outputs
- Problems identified and solutions implemented
- Key insights discovered during implementation
- Any debugging steps taken and findings

## CURRENT IMPLEMENTATION STATE
Exact technical situation right now:
- What's working vs what's broken/incomplete
- Active work in progress (half-finished implementations)
- Current file states and modifications pending
- Any compilation/runtime issues encountered
- Dependencies or prerequisites that are ready/missing

## REQUIRED FILE CONTEXTS
CRITICAL: List ALL files needed as context using EXACT format below. This will be automatically parsed to load file contents.

**MANDATORY FORMAT - Use code block with exact pattern:**
```
filename:startline:endline
filename:startline:endline
filename:startline:endline
```

**PARSING REQUIREMENTS:**
- Each line must be exactly: filepath:number:number
- No spaces around colons
- Use absolute paths from project root (src/main.rs not ./src/main.rs)
- Line numbers must be positive integers
- Start line must be ≤ end line
- End line must be ≤ 10000
- Maximum 10 file ranges total

**INCLUDE THESE FILES:**
- Core implementation files with key functions/classes
- Configuration files with relevant sections
- Test files if testing is involved
- Any modified or newly created files
- Files containing error patterns or debugging areas

**EXAMPLE CORRECT FORMAT:**
```
src/session/chat/session_continuation.rs:100:200
src/config/mod.rs:50:100
tests/integration_test.rs:1:50
```

**WRONG FORMATS (will not be parsed):**
- src/main.rs : 1 : 50 (spaces around colons)
- ./src/main.rs:1:50 (relative path with ./)
- src/main.rs lines 1-50 (text description)
- src/main.rs:1:50, src/lib.rs:1:100 (comma separated)

## IMMEDIATE NEXT STEPS
Specific actionable steps to continue (in order):
- Exact next implementation tasks
- Files to modify and what changes to make
- Commands to run or tools to execute
- Testing or verification steps needed
- Expected challenges and how to handle them

## CRITICAL TECHNICAL DETAILS
Essential information for seamless continuation:
- Important variable names, function signatures, or data structures
- Key algorithms or logic patterns being used
- Error handling approaches or edge cases discovered
- Performance considerations or constraints
- Integration points with existing systems
- Any architectural decisions made and why

## CONTEXT FOR UNDERSTANDING
Background information needed to work effectively:
- How this work fits into the larger system
- Related components or dependencies involved
- Previous attempts or approaches that didn't work
- Domain knowledge or business logic relevant
- Any user requirements or constraints to remember

PROVIDE COMPLETE DETAILS - imagine explaining to a new developer who needs to pick up exactly where you left off.
"#;

pub const CONTINUATION_USER_MESSAGE_TEMPLATE: &str = r#"Thank you for the summary.

Currently we are working on the following requests:
<tasks>
{}
</tasks>

Here's the required file context:
<files>
{}
</files>

Let's continue our work from where we left off.
Please proceed with the next steps as outlined in your summary.
CRITICAL: use tool calling in paralell when its possible to reach results faster and more efficiently."#;
