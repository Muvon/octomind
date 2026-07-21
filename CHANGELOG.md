# Changelog

## [0.38.1] - 2026-07-21

### 📋 Release Summary

This release introduces purpose-based routing for OctoHub, enabling more precise AI model selection based on the specific task at hand (579f9084, 055c5a65). Additionally, the system now defaults to `octohub:auto` for a more streamlined setup experience, supported by updated dependencies and clarified documentation (7c5915e3, bcca4738, da402296, f6083bb1).


### ✨ New Features & Enhancements

- **providers**: split supervisor into granular purposes `579f9084`
- **providers**: implement purpose-based routing for octohub `055c5a65`

### 🔧 Improvements & Optimizations

- **config**: set default models to octohub:auto `7c5915e3`

### 📚 Documentation & Examples

- **providers**: clarify OctoHub purpose routing `bcca4738`

### 🔄 Other Changes

3 maintenance, dependency, and tooling updates not listed individually.

## [0.38.0] - 2026-07-19

### 📋 Release Summary

This release introduces a new device-authorization login flow and comprehensive account and usage management (9d2dc723, 8f90d4d9, 5745e3c0, 06fd829a). The AI supervisor has been enhanced with improved task planning, throughput tracking, and refined tool result handling (3405bf03, 5dc82b17, e2653e22, 0ef7d22c). Additionally, several updates improve session stability, provider reliability, and documentation for MCP tools (e4b3ef40, 80c9e71a, 31f48f6e, e59ea8fa).


### ✨ New Features & Enhancements

- **auth**: implement machine-specific device IDs `9d2dc723`
- **account**: implement account and usage management `8f90d4d9`
- **auth**: implement device-authorization login flow `5745e3c0`
- **config**: add user-scope .env file support `06fd829a`
- **supervisor**: add throughput tracking to stats `3405bf03`
- **supervisor**: make tool result masking optional `5dc82b17`
- **supervisor**: add mid-turn plan recitation `e2653e22`
- **session**: implement stale tool result masking `661855cc`
- **supervisor**: refine gate prompt for observe-only tasks `0ef7d22c`

### 🔧 Improvements & Optimizations

- **config**: fix doc comment formatting `0e620c2c`
- **account**: remove unused clear_session function `81cc5a12`
- **session**: reuse max_retries for empty completions `ffb082cd`
- **bench**: advance baseline to the masking-free build (70%, $3.28) `9d7592d8`
- **session**: remove stale tool result masking `d91bc213`
- **supervisor**: remove mid-turn recitation logic `007aa694`
- **bench**: update baseline results for 0.37.1 `d00b8b8f`
- **oauth**: migrate to file-based token storage `66f18c6b`

### 🐛 Bug Fixes & Stability

- **session**: recite plan regardless of open tasks `e4b3ef40`
- **supervisor**: stop reciting plan when no tasks open `218c1e87`
- **session**: prevent masking of subagent results `80c9e71a`
- **session**: retry empty provider completions `31f48f6e`

### 📚 Documentation & Examples

- update mcp tools and structured output guide `e59ea8fa`

### 🔄 Other Changes

2 maintenance, dependency, and tooling updates not listed individually.

## [0.37.1] - 2026-07-13

### 📋 Release Summary

This release introduces observational verification for the supervisor and adds detailed spending tracking for compression models (d9614d2c, baf3e1d5). Additionally, updates to MCP tool parameters and refreshed performance benchmarks ensure improved system reliability and accuracy (cd5c9065, 862bca89).


### ✨ New Features & Enhancements

- **session**: track compression model spending `baf3e1d5`
- **supervisor**: implement observational verification `d9614d2c`

### 🔧 Improvements & Optimizations

- **mcp**: extract ACP params to validated functions `cd5c9065`
- **bench**: refresh baseline results for 15 instances `862bca89`

## [0.37.0] - 2026-07-12

### 📋 Release Summary

This release improves session management through enhanced conversation compression and more reliable plan completion verification (c8515976, e2f75144). Additionally, general system stability and performance have been upgraded via dependency updates and internal refactoring (8749f30d).


### ✨ New Features & Enhancements

- **supervisor**: enhance session compression and verification `c8515976`
- **supervisor**: implement plan completion verification `e2f75144`

### 🔄 Other Changes

1 maintenance, dependency, and tooling update not listed individually.

## [0.36.1] - 2026-07-12

### 📋 Release Summary

This release includes an update to the core library to ensure improved system stability and performance (93440965). Additionally, internal configuration refinements have been made to streamline overall efficiency (6c4f4f0a).


### 🔧 Improvements & Optimizations

- **config**: remove unnecessary reference to match string `6c4f4f0a`

### 🔄 Other Changes

1 maintenance, dependency, and tooling update not listed individually.

## [0.36.0] - 2026-07-11

### 📋 Release Summary

This release introduces standardized agent instructions via AGENTS.md and provides greater control over MCP tool usage during completions (f57c4091, d371a023). WebSocket reliability has been improved through better request tracking and synchronization (92e77c50, 278bfdd7). Additionally, several updates enhance overall system stability, dependency performance, and session error handling (06d2f211, 0b77195a, e70e9dc0, 877344fe).


### ✨ New Features & Enhancements

- **config**: adopt AGENTS.md standard for instructions `f57c4091`
- **providers**: allow disabling MCP tools in completions `d371a023`
- **ws**: implement request correlation and acknowledgments `92e77c50`

### 🐛 Bug Fixes & Stability

- **websocket**: prevent client hang on command completion `278bfdd7`
- **session**: error on empty provider completions `06d2f211`

### 🔄 Other Changes

3 maintenance, dependency, and tooling updates not listed individually.

## [0.35.0] - 2026-07-03

### 📋 Release Summary

This release enhances the supervisor's intelligence through improved agent profiling, tool output condensation, and a new evidence ledger for better verification (236df27a, 0b969d55, a749e87c). Workflow capabilities have been expanded to allow forced skill assignments and more detailed failure reporting (94004648, 2e83c179). Additionally, several updates improve overall system stability, concurrency, and session management (01d686df, fbc97d0e, fe8dad9d).


### ✨ New Features & Enhancements

- **supervisor**: implement cached agent profile distillation `236df27a`
- **supervisor**: implement task-aware tool output condensation `0b969d55`
- **workflow**: allow forcing skills and capabilities `94004648`
- **supervisor**: implement evidence ledger for verification `a749e87c`
- **workflow**: capture stderr for process failures `2e83c179`

### 🔧 Improvements & Optimizations

- **bench**: refresh baseline metrics for v0.35.0 `0d399de6`
- **websocket**: simplify connection state management `77eac971`
- **supervisor**: implement round-based signal aggregation `fc04a525`

### 🐛 Bug Fixes & Stability

- **supervisor**: handle newlines in file backend storage `fe8dad9d`
- **core**: resolve stability, leak, and concurrency issues `01d686df`
- **session**: reset gate iterations on new task `fbc97d0e`

### 🔄 Other Changes

4 maintenance, dependency, and tooling updates not listed individually.

## [0.34.1] - 2026-06-27

### 📋 Release Summary

This release improves session stability by optimizing how AI plans are processed and managed (a5fca6bf). Additionally, general system updates and dependency upgrades enhance overall performance and reliability (be4e7761, df9c10c8).


### 🔧 Improvements & Optimizations

- **session**: format instruction content string `df9c10c8`

### 🐛 Bug Fixes & Stability

- **session**: defer plan compression until turn acceptance `a5fca6bf`

### 🔄 Other Changes

1 maintenance, dependency, and tooling update not listed individually.

## [0.34.0] - 2026-06-27

### 📋 Release Summary

This release enhances codebase interaction through improved embedding strategies and more efficient text chunking (bc4a4f03, b5b66c2d, d890d98a). The AI supervisor is now more refined with adaptive steering and updated system reminders for better focus and reliability (2054d424, ab6a69d5, ac83c711, bc94f535). Additional updates include new regression testing tools and general stability improvements across the platform (acc39bce, 82884dd8, f38a5256, b05dda05).


### ✨ New Features & Enhancements

- **bench**: add baseline regression testing tools `acc39bce`
- **supervisor**: implement adaptive steer backoff `2054d424`
- **session**: add system reminders to prompt `ab6a69d5`
- **embeddings**: implement token-based text chunking `bc4a4f03`
- **embeddings**: implement recursive chunking and optimize inference `b5b66c2d`
- **embeddings**: invalidate cache on model revision change `d890d98a`

### 🔧 Improvements & Optimizations

- **mcp**: remove unused id utility and refine prompt `b05dda05`
- **supervisor**: update system reminders to pay-attention `bc94f535`

### 🐛 Bug Fixes & Stability

- **supervisor**: prevent sequential steer spam `ac83c711`
- **embeddings**: resolve model cache path lookup `82884dd8`

### 🔄 Other Changes

1 maintenance, dependency, and tooling update not listed individually.

## [0.33.1] - 2026-06-21

### 📋 Release Summary

This release improves agent coordination through enhanced subagent handoffs and clearer plan recitations (3f038c09). Additionally, the supervisor now provides more detailed tracking of steering signals to optimize AI guidance (fd4b1360).


### ✨ New Features & Enhancements

- **supervisor**: track steer breakdown by signal `fd4b1360`
- **mcp**: enhance subagent handoff and plan recitation `3f038c09`

## [0.33.0] - 2026-06-20

### 📋 Release Summary

This release introduces advanced supervisor intelligence to improve goal tracking, tool execution accuracy, and distraction detection (f4578584, 3d113677, 4ac1db0b, cbdd8e1a, 807587d8, 4b80a8f1, 63a58570), alongside an orchestration MCP server and token efficiency benchmarking (9efbee9a, 69c8156a). User session management has been enhanced through improved message handling and refined tool response deduplication (7881570b, 0659f81b, cd425077). Additionally, several updates optimize system steering, data formatting, and overall stability (24164258, 53de784b, 4bb231bf, be51537a, 735ddaf6).


### ✨ New Features & Enhancements

- **bench**: add token efficiency benchmarking suite `69c8156a`
- **session**: implement system-managed user messages `7881570b`
- **supervisor**: add sequential tool call detection `f4578584`
- **steering**: implement rotating framing for signals `24164258`
- **supervisor**: implement steer circuit-breaker `3d113677`
- **supervisor**: implement distraction detection logic `4ac1db0b`
- **supervisor**: implement tool call deduplication `cbdd8e1a`
- **session**: move tool deduplication upstream `0659f81b`
- **config**: add orchestration MCP server `9efbee9a`
- **supervisor**: implement deterministic claim verification `807587d8`
- **supervisor**: implement goal recitation and mutation gating `4b80a8f1`
- **supervisor**: implement tool truncation detection `63a58570`

### 🔧 Improvements & Optimizations

- **supervisor**: refine steering and gating logic `ece19ae5`
- **utils**: improve truncation notice clarity `f531eaa6`
- **supervisor**: improve truncation guidance note `ffe4631c`
- **supervisor**: replace relevance with drift detection `10c1fd56`
- **mcp**: migrate tap and schedule to orchestration `6edfc7cb`
- **mcp**: migrate core tools to runtime module `bc7278b4`
- **schedule**: clarify recurring loop behavior `db15f6cf`

### 🐛 Bug Fixes & Stability

- **supervisor**: prevent greedy quote stripping `53de784b`
- **supervisor**: resolve YAML escaping asymmetry `4bb231bf`
- **supervisor**: consolidate steering signals in parallel batches `be51537a`
- **compression**: exclude synthetic user messages from tasks `735ddaf6`
- **session**: refine tool response deduplication `cd425077`

## [0.32.0] - 2026-06-17

### 📋 Release Summary

This release introduces advanced workflow capabilities, including parallel execution, dynamic fan-out, and structured JSON/JSONL output support (d9717687, 3155ca29, d4350ef5, acd7f390, f0c010ae). Users can now better manage costs with global spending caps and benefit from improved MCP tool reliability and session persistence (aa298801, 2cb52cf7, 864864b7, d5ddaf38). Additionally, several bug fixes enhance system stability, specifically regarding guardrail execution and resource handling (5f44be41, 5a99b98a, c0b3fe34).


### ✨ New Features & Enhancements

- **structured-output**: add JSON schema support `f0c010ae`
- **workflow**: implement jsonl output for artifacts `d9717687`
- **workflow**: implement dynamic parallel fan-out `3155ca29`
- **workflow**: implement parallel fan-out and aggregation `d4350ef5`
- **workflow**: add max_cost spending cap for the whole run `aa298801`
- **mcp**: enhance tool response truncation handling `2cb52cf7`
- **compression**: persist active task across compactions `d5ddaf38`
- **workflow**: implement tap-based workflow discovery `48ae3b97`
- **workflow**: implement per-step JSONL streaming `4664c03d`
- **workflow**: add jsonl output format `acd7f390`
- **mcp**: include local servers in info output `864864b7`
- **workflow**: add working directory support for steps `80c73ae5`

### 🔧 Improvements & Optimizations

- **workflow**: format token subtraction logic `8d09fa7c`
- **docker**: dynamicize image versioning fix(schedule): improve duration parsing errors `1e4e0840`
- **docker**: add build workflow and fix sg installation `5392a1c8`

### 🐛 Bug Fixes & Stability

- **guardrails**: reject duplicate validator/pipe names at load `5f44be41`
- **websocket**: reset per-request spending checkpoint each turn `c0b3fe34`
- **workflow**: count continue-session cost/tokens once (fixes max_cost cap) `34d4798f`
- **mcp**: preserve is_error through large tool-result truncation `7afc89e8`
- **guardrails**: prevent turn hang and orphaned processes in hook/validator/pipe spawners `5a99b98a`
- **workflow**: allow built-in placeholders in step prompts `a3c8b255`

### 🔄 Other Changes

2 maintenance, dependency, and tooling updates not listed individually.

## [0.31.0] - 2026-06-12

### 📋 Release Summary

This release introduces the Claude Fable 5 provider, secure SQLite-backed credential storage, and an `/agents` command for improved session monitoring (01c63ccb, 09d7b33a, 743cf434). Significant enhancements were made to the supervisor and MCP systems, including real-time task tracking, advanced loop detection, and more efficient parallel tool execution (3167d50f, fac5635e, fc6b2aba, a13ce968). Various bug fixes further improve system stability, cost tracking accuracy, and server reliability (fabbed65, 76eb378d, 42d4006f).


### ✨ New Features & Enhancements

- **mcp**: implement real-time tracking for tap runs `3167d50f`
- **chat**: enhance animation manager and status notifications `d368088c`
- **anthropic**: add Claude Fable 5 provider `01c63ccb`
- **auth**: implement SQLite-backed credential storage `09d7b33a`
- **supervisor**: add detailed call statistics breakdown `11292f6d`
- **supervisor**: enhance loop detection and reinforcement `fac5635e`
- **supervisor**: implement out-of-band control plane `fc6b2aba`
- **agents**: add token and cost tracking to views `74a5868e`
- **mcp**: set background execution as default for run `f3a646a5`
- **session**: improve parallel tool call instructions `a13ce968`
- **agents**: implement /agents command for monitoring `743cf434`

### 🔧 Improvements & Optimizations

- **github**: migrate rust and release jobs to shared workflows `af044141`
- **windows**: resolve MSVC runtime library mismatch `dc773b90`
- **acp**: add lifecycle and EOF signaling tests `abb29d08`
- **acp**: implement actor bridge and update deps `260bca75`
- **docker**: implement multi-platform builds and optimization `b6b9ba14`
- **mcp**: delegate project id derivation to octolib `6ee61f4a`
- **compression**: use enforces_response_schema check `14825b62`
- **session**: format debug_assert in compression logic `fba071b7`
- **compression**: implement round-robin ratio selection `12de5c64`
- **session**: simplify parallel tool call prompt `a92e4866`

### 🐛 Bug Fixes & Stability

- **mcp**: prevent EPIPE in fake server tests `465398f1`
- **acp**: ensure deterministic shutdown on stdin EOF `903fbfd0`
- **session**: improve tool result deduplication and cost logging `fabbed65`
- **supervisor**: prevent leakage of echoed state placeholders `2232bf1a`
- **supervisor**: improve self-report parsing and stripping `9b88440a`
- **mcp**: prevent restart loop for failed servers `76eb378d`
- **mcp**: stop exposing fallback tools for offline servers `42d4006f`
- **mcp**: remove credentials from git remote URLs `05560b82`
- **plan**: prevent orphaned tool calls during compression `e3ab7c7c`

### 📚 Documentation & Examples

- **readme**: correct developer agent command examples `d219e42e`

### 🔄 Other Changes

3 maintenance, dependency, and tooling updates not listed individually.

## [0.30.0] - 2026-06-03

### 📋 Release Summary

This release introduces advanced AI-driven conversation compression, a new lesson management system for learning, and robust project-local guardrails to ensure higher quality interactions (4e32734b, 8a269847, f917b4ad). Users can now leverage an external workflow orchestrator, session sharing, and improved terminal controls for a more flexible development experience (e7b56c13, 3a1e12cc, 12818db1). Significant stability improvements address MCP tool call sequencing, session persistence, and general system reliability (ac22fc5d, d27da3d3, bdcfe77b).


### 🚨 Breaking Changes

⚠️ **Important**: This release contains breaking changes that may require code updates.

- **workflow**: route all outputs to stderr `277f1ce1`
- **session**: remove provider field from session info `fc1289ad`

### ✨ New Features & Enhancements

- **session**: integrate handle_done into main loop `a06b9d41`
- **acp**: implement /done command with instructions `a9466cf3`
- **learning**: add global scope support to lessons `639c23fb`
- **learning**: implement global scope and tiered recall `faa0ff3a`
- **session**: implement persistent command logging `6aafd225`
- **workflow**: implement standalone TOML execution `89bd7ac3`
- **session**: implement zstd compression for logs `3cc45f41`
- **workflow**: expand prompts with system placeholders `065ae7aa`
- **workflow**: add markdown rendering to step outputs `4cf48565`
- **workflow**: implement per-step model overrides `1c6b9a8b`
- **workflow**: track and display tool failure statistics `a04ba41b`
- **term**: implement tty echo suppression and fd control `12818db1`
- **workflow**: enhance step rendering and execution UI `65911396`
- **config**: add develop workflow template `3e31309e`
- **workflow**: migrate internal workflows to external CLI `a74aab7f`
- **workflow**: implement contextual prompts for continue sessions `c5a7cc71`
- **workflow**: implement external workflow orchestrator `e7b56c13`
- **config**: add toggle for auto-activating capabilities `d39d761b`
- **compression**: implement XML fallback for structured output `1aa8e221`
- **chat**: improve /done command feedback `707a3dec`
- **chat**: hide system tags in welcome messages `ac519131`
- **compression**: migrate to structured JSON and XML formats `94564637`
- **compression**: implement synthetic continuation wrappers `185a17bc`
- **guardrails**: implement post-turn validator scripts `632a09e7`
- **guardrails**: implement post-tool execution hooks `a8ae170c`
- **session**: implement project-local guardrails system `f917b4ad`
- **session**: track tool calls in assistant messages `6ac4a06e`
- **session**: persist chat session role `55cc8e50`
- **session**: warn about local share URLs and add tips `7ca5bde7`
- **session**: implement /analyze command and local bridge `7b272387`
- **session**: implement session sharing functionality `3a1e12cc`
- **session**: add critical knowledge to plan display `3a44fe0c`
- **schedule**: implement idle-based message triggering `c9773625`
- **session**: improve content cache marker management `610f150b`
- **learning**: implement lesson management system `8a269847`
- **compression**: allow compression when ignoring costs `a622d1c0`
- **acp**: persist session info after messages `8ddf32ee`
- **compression**: implement cost-aware decision logic `a61f333e`
- **session**: implement conversation compression `4e32734b`
- **session**: implement AI-driven conversation compression `2f4fd29d`
- **compression**: implement advanced conversation compression `3d4f6d83`

### 🔧 Improvements & Optimizations

- **session**: remove obsolete cache marker logic `b8225527`
- **test**: reformat write_zstd_line calls `3da9a4fa`
- **session**: simplify session file writing in tests `27a0fe34`
- **workflow**: improve execution log visualization `2b588704`
- **core**: clean up whitespace and formatting `c4d8813c`
- **mcp**: clarify discovery flow requirements `02ed57f7`
- **config**: set gpt-5-mini as default compression model `f1a6e86e`
- **session**: optimize conversation compression prompts `8aa15ba4`
- **compression**: remove first_prompt_idx tracking `6a825609`
- **guardrails**: rename rules to guards and fix session logging `33884349`
- **guardrails**: clarify history-based rule tests `0182aac3`
- **session**: add role field to session mock data `0844f491`
- **session**: replace provider field with role `9cd65d0b`
- **session**: improve report and list formatting `19104886`
- **cli**: implement structured block rendering for output `6f8c0edb`
- **mcp**: lower routing accuracy threshold to 80% `efdc097a`
- **mcp**: recalibrate semantic thresholds for fine-tuned model `de8eb12b`
- **learning**: simplify file deletion logic `52a9dba7`
- **compression**: remove unused imports `4f974721`
- **compression**: decouple range logic to new module `8bbd24f0`
- **compression**: decouple knowledge handling `f5303b31`
- **mcp**: remove unused tool call parsing fallback `6e82aa19`
- **mcp**: remove legacy is_server_already_running `4154d042`
- **session**: remove legacy layers enabled state `5b22c856`
- **session**: encapsulate runtime initialization `85c071cb`
- **mcp**: centralize background job termination `8b8287c9`
- **learning**: consolidate lesson extraction logic `1f206b0d`
- **session**: extract keepalive drain logic `b4b378cc`
- **config**: simplify caching and remove MCP warnings `04e3025b`
- **session**: streamline llm calls and compression tests `45a6e9a1`
- **mcp**: stabilize connection type serialization `736613f7`
- **session**: implement typed cancellation errors `89fa6aa2`
- **session**: replace string parsing with GenericSessionArgs `f8fa078b`
- **workflow**: migrate pr brief to reusable workflow `4cba0b35`

### 🐛 Bug Fixes & Stability

- **agent**: improve tap loading and cloning errors `1b7b9851`
- **session**: suspend animation during idle prompt `edb8a623`
- **mcp**: prevent invalid tool call sequences during compression `ac22fc5d`
- **chat**: handle transient cursor position report failures `f22be27c`
- **mcp**: prevent orphaned tool calls during compression `50177ab3`
- **session**: prevent MCP tool ownership errors during role swap `0cdf3b50`
- **mcp**: prevent deadlock on stdin server timeout `8c560499`
- **session**: recover JSON from AI text content `fd314250`
- **mcp**: restart dead stdin servers during tool calls `bdcfe77b`
- **compression**: prevent invalid tool use sequence `03b97a0b`
- **chat**: show tool preview for single tool calls `b23ee611`
- **compression**: prevent ratio wrap-around during escalation `588b09c2`
- **session**: skip credential check for cli provider `81c8bc0a`
- **session**: log errors when appending to session file `baa8246f`
- **config**: prevent panic on missing role lookup `06862ce7`
- **session**: log errors during chat session save `d27da3d3`
- **session**: prevent panic on tokenizer initialization failure `94d72bb0`
- **providers**: prevent panic on malformed tool calls `c9c34007`
- **session**: propagate errors in non-daemon mode `bf865767`
- **session**: preserve assistant response id during compression `48e6e3c4`
- **chat**: align compression log style with cost display `33abefba`

### 📚 Documentation & Examples

- comprehensively expand project and protocol documentation `2a521251`
- **config**: set gpt-5-mini as default compression model `6f1eef92`
- add contribution guidelines `4435f767`
- **mcp**: clarify session requirement for run action `6088a1e4`
- **instructions**: remove redundant workflow guides `19ad243a`
- **readme**: expand pillars to include guardrails and intent-driven context `2ce0e98d`
- **config**: document BINARIES variable `278f7774`
- **general**: bump rust version and refine guides `99b34200`
- **usage**: add guardrails documentation `80c39781`
- comprehensive update to developer and user guides `555e969c`

### 🔄 Other Changes

7 maintenance, dependency, and tooling updates not listed individually.

## [0.29.0] - 2026-05-15

### 📋 Release Summary

This release introduces advanced session management features, including a new `/schedule` command for recurring tasks, automated tool activation based on user intent, and parallel tool execution for faster workflows (4015ef10, 223d370b, 4a2da242). The user interface has been significantly refined with a continuous input rail, real-time cost tracking, and enhanced markdown rendering to provide a more seamless conversational experience (ae7bc882, e2e4c0bb, 8bdd636e). Additionally, the update integrates a local embedding engine for improved codebase context and resolves several stability issues related to terminal rendering and session race conditions (b0d22268, 28d9d18e, d6115db5).


### ✨ New Features & Enhancements

- **chat**: add continuous left rail for user input `ae7bc882`
- **assets**: add animated octomind brand assets `51c285d3`
- **chat**: enhance tool execution and cost display UI `10751d00`
- **learning**: integrate local embedding engine and update architecture `b0d22268`
- **chat**: add persistent status line with cost delta `e2e4c0bb`
- **chat**: unify session status and context tracking `6e1c593d`
- **chat**: highlight submitted user input in history `72183103`
- **schedule**: implement persistence and state restoration `94203409`
- **agent**: implement domain-based capability filtering and gating `da1518dc`
- **embeddings**: implement persistent vector cache and pre-embedding `d0a53534`
- **mcp**: enhance auto-activation logic and coverage reporting `3ada7337`
- **session**: instruct model to use parallel tool calls `4a2da242`
- **session**: enhance session management and system prompt structure `f9e8279b`
- **mcp**: implement cross-encoder reranking for auto-activation `195242b1`
- **mcp**: add detailed logging for auto-activation `a81a7b9c`
- **acp**: implement inbox message streaming and injected events `2d84ce00`
- **session**: display injected inbox messages `38c3aa7f`
- **schedule**: implement auto-processing and implicit scheduling `6ff07764`
- **mcp**: implement recurring schedules and /schedule command `4015ef10`
- **mcp**: implement intent-based capability auto-activation `223d370b`

### 🔧 Improvements & Optimizations

- **ui**: add visual separators for assistant output `2f9b854e`
- **branding**: redesign logo and simplify icon rendering `61975c51`
- **embeddings**: serialize embedding model tests `af640a54`
- **session**: simplify delegation prompt text `a144cd78`
- **embeddings**: replace fastembed with candle and octomind-embed `628b74a5`
- **embeddings**: remove cross-encoder reranking logic `ef37f18e`
- **acp**: extract connection client to variable `8d9a9670`
- **cargo**: point homepage to octomind.run domain `7a3eaa11`
- **docker**: add g++ to build dependencies `490ac1b9`

### 🐛 Bug Fixes & Stability

- **acp**: resolve session race conditions and image alignment `d6115db5`
- **acp**: prevent panic on done command `df121eed`
- **chat**: prevent spinner residue during markdown render `8bdd636e`
- **chat**: resolve terminal rendering and deadlock issues `28d9d18e`
- **session**: improve tool execution CLI output formatting `fc6e080c`
- **chat**: implement plain status for spinner `fa38e09e`
- **session**: suppress Ctrl+C echo in terminal `05a5d73f`
- **mcp**: clarify capability action prompt usage `7af77768`
- **config**: set default agent to concierge `cae4f63d`
- **plan**: improve task compression and tool call preservation `8ad0beaa`

### 🔄 Other Changes

2 maintenance, dependency, and tooling updates not listed individually.

## [0.28.0] - 2026-05-10

### 📋 Release Summary

This release significantly expands the Model Context Protocol (MCP) ecosystem with support for dynamic tool registration, project-local shebang-based tools, and improved subprocess execution (1f17e0d3, 2efac8ca, 3bc76283, 0c30134d). Users can now monitor token usage and costs directly via metadata, while chat interactions benefit from refined markdown detection and streamlined session context management (bb0aeed0, 312e1d06, 73fde53f). Additional updates include optimized Docker builds and improved tool parsing to ensure a more stable and responsive development experience (670d0291, 869b482c).


### ✨ New Features & Enhancements

- **mcp**: support dynamic tool registration via runtime overlay `1f17e0d3`
- **acp**: report token usage and cost via meta `bb0aeed0`
- **mcp**: implement project-local shebang-based tools `2efac8ca`
- **mcp**: implement runtime tool overlays for dynamic expansion `3bc76283`
- **mcp**: stream subprocess updates and downgrade windows-sys `e8609967`
- **mcp**: delegate tap-run execution to external ACP subprocess `0c30134d`
- **docker**: include assets directory in build stage `670d0291`

### 🔧 Improvements & Optimizations

- **session**: remove manual context management commands `73fde53f`

### 🐛 Bug Fixes & Stability

- **mcp**: decouple tool parsing from server filtering `869b482c`
- **config**: serialize runtime overlay tests `511e3832`
- **chat**: improve markdown detection heuristics `312e1d06`

### 🔄 Other Changes

1 maintenance, dependency, and tooling update not listed individually.

## [0.27.0] - 2026-05-09

### 📋 Release Summary

This release introduces advanced reasoning controls with the `/effort` command and enhances the development experience through semantic skill activation, hybrid search capabilities, and improved multimodal support for video content (b393e5b9, fe4d334e, 5d7f68d3, c5df2eaa). System efficiency is significantly improved via intelligent prompt caching, automated session compaction, and a new cost-tracking mechanism for real-time API pricing (0356fa70, 095a58e0, 2fe619b0). Stability is further bolstered by refined retry logic for API failures, atomic message persistence, and a refreshed visual identity across the CLI (3a121767, 4142dff8, 1f5b8ee2).


### ✨ New Features & Enhancements

- **session**: add /effort command for reasoning control `b393e5b9`
- **llm**: add reasoning effort configuration `aecf4807`
- **session**: add prompt cache keepalive mechanism `0356fa70`
- **chat**: prioritize video file URLs in clipboard `c5df2eaa`
- **mcp**: implement margin gate for semantic skill activation `ddfe1fd8`
- **mcp**: implement progress tracking for environment capabilities `9edba62f`
- **mcp**: implement deterministic and dynamic capability loading `5030df75`
- **core**: split runtime server and add tap tool `77065240`
- **agent**: prevent stdin deadlocks in subagents `6174f1fa`
- **mcp**: implement tap agent discovery and background execution `b2428aea`
- **session**: enforce concise output constraints `549f1a66`
- **chat**: wrap multiline pastes in log tags `ec99cafc`
- **cost_tracker**: use real per-token pricing `2fe619b0`
- **mcp**: implement refcounted shared server management `ecf7119c`
- **core**: implement dynamic capability discovery and activation `cb8b678f`
- **chat**: add retry mechanism for failed API requests `3a121767`
- **schedule**: add support for repeating tasks `77526e75`
- **branding**: implement visual identity and CLI startup banner `1f5b8ee2`
- **acp**: add CLI overrides for session management `bcc48c70`
- **mcp**: support namespaced tool filters in capabilities `8c6e924f`
- **mcp**: clarify tool availability in activation response `e29e21f3`
- **mcp**: support activation of deps-only capabilities `0b2e83f6`
- **skill**: implement semantic activation triggers `fe4d334e`
- **learning**: implement hybrid search with RRF `5d7f68d3`
- **mcp**: implement LRU eviction for active capabilities `8cb730b7`
- **capability**: deterministic auto-activation via trigger embeddings `508d9c34`
- **embeddings**: update model paths and enhance session tracking `322eb61e`
- **mcp**: add capability discovery and model warmup `11cea454`
- **plan**: wire compaction anchor into task-level compression `6e0aae30`
- **session**: add anchor data model for iterative compaction summaries `095a58e0`
- **session**: deduplicate identical tool results within a session `20f479cb`
- **capability**: use embedding cosine for discover with keyword fallback `87ec231e`
- **embeddings**: add internal embedding module backed by octolib fastembed `3471f00a`
- **mcp**: expose capability as a runtime tool `faace7c1`

### 🔧 Improvements & Optimizations

- **mcp**: simplify and standardize tool descriptions `a9b9fe02`
- **workflow**: link libgcc for musl static builds `a1959845`
- **workflow**: build re2 target for static ORT `3a17c752`
- **workflow**: fix static linking for ONNX Runtime `16769806`
- **workflow**: verify full onnx build via re2 check `115d450d`
- **assets**: redesign logo and icon assets `22adece1`
- **github**: fix windows linking and musl search paths `b2caf7b6`
- **workflow**: correct ORT_LIB_LOCATION for static linking `458ef8c8`
- **branding**: reformat grid and cell logic `6dd124e3`
- **github**: optimize onnxruntime static linking for linux and windows `32dd480a`
- **musl**: bust stale ORT cache key to force clean rebuild `dd4e2032`
- **github**: switch to powershell for windows runner `c009ec0a`
- **workflow**: add Windows static ONNX Runtime build `21914f46`
- **github**: persist static library via volume mount `185487c8`
- **github**: relocate onnxruntime build artifacts `7e5f30ed`
- **workflow**: preserve library directory during build `b53bef94`
- **learning**: move lesson extraction to background tasks `31207910`
- **github**: add ONNX Runtime static lib download `4429f6f1`
- **github**: add static ONNX Runtime build for musl `f88b419c`
- **agent**: reformat code for consistency `4d51a27f`

### 🐛 Bug Fixes & Stability

- **chat**: wrap validation errors in xml tags `62adb157`
- **chat**: prevent history truncation on tool follow-up errors `43603a61`
- **session**: exclude tool errors from deduplication `c48e08ad`
- **session**: improve retry logic for follow-up API failures `1c45366b`
- **chat**: ensure atomic message persistence across session types `4142dff8`
- **mcp**: prevent false-positive skill activation `fb7d6e97`
- **branding**: snap icon rects to design grid `6b08b81a`
- **chat**: prevent context loss from empty summaries `e41805fb`
- **build**: create workspace dir for musl targets `27f65eda`
- **chat**: ensure spinner cleanup and handle cancellation `d494f6e2`

### 📚 Documentation & Examples

- **readme**: synchronize model names and commands `5bc0ee00`
- **readme**: restructure pillars and update branding `250590e5`
- **mcp**: explain split between core and runtime `78aec1b2`
- **instructions**: update project structure and code patterns `bb393a80`
- **readme**: rewrite content to focus on three pillars `848587d7`

### 🔄 Other Changes

8 maintenance, dependency, and tooling updates not listed individually.

## [0.26.0] - 2026-04-28

### 📋 Release Summary

This release introduces expanded AI model support for Featherless, NVIDIA NIM, Groq, and BytePlus, alongside enhanced multimodal capabilities including clipboard image pasting and optimized media previews (6cf59b30, 4dba526f, 494df4d9, 287baeba). The session experience is significantly improved through a refined skill persistence system during context compression, unified MCP tool management, and new real-time performance metrics for token usage and throughput (333248d5, 3e4dded7, 92efb57a, 42abaa45). System stability is further bolstered by streamlined configuration validation, more robust skill lifecycle management, and several fixes to tool parameter handling and session naming (77fd2b3f, f6464a32, 5d697741).


### ✨ New Features & Enhancements

- **provider**: add featherless support via octolib `6cf59b30`
- **chat**: add token usage averages to info command `92efb57a`
- **chat**: add throughput metrics to info command `42abaa45`
- **session**: enhance skill persistence during compression `333248d5`
- **core**: enhance config validation and session naming `77fd2b3f`
- **octolib**: support Groq, BytePlus, and new AI models `e4ed32f3`
- **proctitle**: add process and terminal title support `3cb24a2f`
- **chat**: add inline image previews for clipboard paste `494df4d9`
- **chat**: intercept paste events for media files `0840301b`
- **chat**: add Ctrl+V support for clipboard media attachments `287baeba`
- add nvidia nim provider support `4dba526f`
- **skill**: emit skill lifecycle events via WebSocket `7fd5eaf5`
- **mcp**: unify server management and add tools retrieval `3e4dded7`
- **config**: support mcp-*.toml override configs and fix auto-bind tracking `eb1e2433`
- **agents**: generalize developer and add skill system `687fd836`
- **chat**: preserve skills during compression `e1f7517e`

### 🔧 Improvements & Optimizations

- **skill**: remove forced compression on skill forget `ddcbb255`
- **core**: migrate layers to ACP protocol and update docs `fe8c245a`
- **session**: shorten and reorder session name components `13a1ca9a`
- **session**: downscale inline image previews `321d00e8`
- **acp**: replace save and cache with skill command `004c56dc`
- **config, layers, session**: restructure layers to roles architecture `d529770f`
- **website**: remove website and deployment logic `9467f35f`

### 🐛 Bug Fixes & Stability

- **config**: require command field for ACP layers `0d99d363`
- **mcp**: ensure MCP function parameters have type field `5d697741`
- **mcp**: honor dynamic server enable/disable per session `ae78b3d3`
- **skill**: prevent duplicates and fix validation `f6464a32`
- **session**: prevent mid-loop tool compression `ff66fd8b`

### 📚 Documentation & Examples

- **session**: clarify skill lifecycle and compression `557e42e0`
- **pipeline**: expand provider, compression, and workflow documentation `6a1e2a4d`
- **map**: sync project map and architecture docs `23996626`

### 🔄 Other Changes

3 maintenance, dependency, and tooling updates not listed individually.

## [0.25.0] - 2026-04-22

### 📋 Release Summary

This release overhauls skill management by introducing declarative activation rules and a new `/skill` command while streamlining the CLI and session recovery process (64ccb7e, 85b8553, a165791, b5b862a). Session reliability is significantly improved through plan persistence, automated skill validation, and robust context compression that prevents data loss and infinite loops (34e46c1, d79630c, aabe16b, f41e547). Additionally, the update strengthens MCP integrations with OAuth discovery support and resolves various UI lags and server deadlocks to ensure a smoother interactive experience (633eef2, 294f760, 914f9cb, f71c760).


### 🚨 Breaking Changes

⚠️ **Important**: This release contains breaking changes that may require code updates.

- **skills**: use declarative activation rules `64ccb7eb`
- **session**: add /skill and remove /save `85b8553b`
- **session**: remove /save and improve logging `0a56bf7d`
- **mcp**: implement declarative skill activation `726c5762`
- **skill**: simplify CLI and session resume `a1657911`
- **skill**: implement toggle-based management `69d41aad`

### ✨ New Features & Enhancements

- **session**: add plan persistence and recovery `34e46c19`
- **agent**: implement tag-based agent resolution `52fa80ab`
- **skill**: add support for universal skill directories `1c09341b`
- **config**: add script timeout and retry options `6917d84d`
- **skill**: expose trigger rules on activation `0bac20c5`
- **skills**: add auto-validation and status info `0d0b6c66`
- **config**: add provider HTTP request timeout `3c9fb840`
- **rules**: add bin, session, and workdir rules `181510d0`
- **skill**: add pagination and glob pattern filtering `5702b5e5`
- **skill**: implement refcounted MCP server management `9ab63bc1`
- **chat**: expand skill automation to all events `af38b9c5`
- **session**: add skill validation and context tags `dcb0d81e`
- **acp**: load env skills on session creation `b207f7a4`
- **skill**: enhance command UI and session setup `864539f1`
- **skill**: implement silent activation and injection `46f9984f`
- **session**: add skills to session entry points `a57d175a`
- **skills**: support environment-based activation `7406651b`
- **mcp**: support full activation for env skills `a27feb57`
- **skills**: implement skill management and /skill command `b5b862aa`
- **mcp**: implement skill automation config limits `a402dbe1`
- **skill**: implement auto-activation and validation `d77f0336`
- **mcp**: implement RFC 9728 OAuth discovery `633eef2c`
- **cache**: add configurable long system cache TTL `c4e341a3`
- **mcp**: add workdir to MCP server initialization context `043ff2cb`

### 🔧 Improvements & Optimizations

- **session**: use is_multiple_of for parity check `db13182e`
- **config**: enable learning and tune limits `0d4ba6d6`
- **chat**: reset animation state `95ab56ee`
- **session**: streamline session logging `8c0d3bb0`
- **session**: improve animation and sorting `9a34c7fa`
- optimize storage and remove mutability `6a46f07c`
- **session**: compress-all strategy with user re-injection `ef150542`
- **ci**: add PR brief generation workflow `474d65fb`

### 🐛 Bug Fixes & Stability

- **session**: persist assistant response to file `d79630c4`
- **session**: persist state after compression `826a494f`
- **compression**: prevent orphaned tool calls `aabe16b4`
- **mcp**: prevent data loss on partial line reads `294f760b`
- **mcp**: prevent deadlocks in server status checks `914f9cbb`
- **chat**: stop spinner before user prompt `90e8d764`
- resolve compression errors and CLI artifacts `ae71cca6`
- **core**: resolve chat lag and update docs `f71c760a`
- **skill_auto**: reset validator retry counters `2d793ba0`
- **deps**: resolve security vulnerabilities `79c3521d`
- **utils**: prevent UTF-8 boundary panics `776e409c`
- **session**: prevent infinite compression loops with escalation `f41e5473`

### 🔄 Other Changes

7 maintenance, dependency, and tooling updates not listed individually.

## [0.24.0] - 2026-04-10

### 📋 Release Summary

This release introduces a cross-session adaptive learning system with automatic lesson extraction and an inbox monitor for improved background session management. Pipeline execution is now deterministic with step timing display, and GitHub token authentication has been added for CI rate limits. Several bug fixes improve stdin handling, UTF-8 character support, and system stability (7bfd8852, 5b120090, edc69aeb, c44be5f9, 9a3a5ddf, c7f239e1, 7d04200b, e2e089db, d28c5bf7, 91859d75).


### ✨ New Features & Enhancements

- **install**: add GitHub token auth for CI rate limits `7bfd8852`
- **acp**: add background inbox monitor for sessions `5b120090`
- **orchestrator**: add step timing and result display `edc69aeb`
- **pipeline**: add deterministic pipeline execution `c44be5f9`
- **learning**: add fire-and-forget lesson extraction on exit `ee725ca6`
- **learning**: add title and octobrain support `66f954c6`
- **learning**: add cross-session adaptive learning system `9a3a5ddf`
- **mcp**: include session_id in MCP server capabilities `53e8f540`

### 🔧 Improvements & Optimizations

- **smoke**: refactor and add smoke tests `5ce17e92`
- **inbox**: replace sleep polling with notify wake-up `c20ff1eb`
- **compression**: differentiate forced /done from automatic compression behavior `d64a579a`
- **learning**: improve extraction with evidence and deduplication `38678da9`
- **learning**: add debug logging for extraction and retrieval `f499f565`
- **session**: remove SIGTSTP truncation point handler `998ba55f`

### 🐛 Bug Fixes & Stability

- **agent**: improve dep script error reporting with stderr output `c7f239e1`
- **run**: read piped stdin before async subprocess spawning `7d04200b`
- **acp**: resolve inbox monitor race conditions `e2e089db`
- prevent UTF-8 truncation splitting multi-byte characters `d28c5bf7`
- **session**: handle interrupted tool calls cleanly `91859d75`

### 📚 Documentation & Examples

- add learning features and CI/CD documentation `af5297f5`
- **readme**: add TOC and detailed usage sections `5604913f`
- **pipelines**: add pipeline feature documentation `5e92fa61`

## [0.23.1] - 2026-04-04

### 📋 Release Summary

This update enhances the reliability of AI responses through improved structured output behavior and optimizes the default agent configuration for more versatile assistance (2b7850c5, cc8dca8f). General maintenance includes comprehensive dependency updates and legal documentation refreshes to ensure continued system stability and compliance (748aeb29, 55f0e4bd, b0f90304).


### ✨ New Features & Enhancements

- **workflow**: add pipeline support for role workflows `e661a700`

### 🔧 Improvements & Optimizations

- **core**: improve structured output behavior `2b7850c5`
- **config**: update default agent to assistant:general `cc8dca8f`

### 🔄 Other Changes

4 maintenance, dependency, and tooling updates not listed individually.

## [0.23.0] - 2026-03-28

### 📋 Release Summary

This release migrates Octomind to a layered architecture, introducing event-driven agents with dynamic MCP server support and daemon mode for background operation. New capabilities include per-agent model overrides, HTTP webhook system for external integrations, shell completion for the run command, and Windows named pipe support for cross-platform messaging. The compression system now uses exponential cooldown for better context retention, while numerous bug fixes improve process management, session isolation, and overall system stability.


### 🚨 Breaking Changes

⚠️ **Important**: This release contains breaking changes that may require code updates.

- migrate to layered architecture and remove legacy docs `f6a95722`

### ✨ New Features & Enhancements

- **config**: allow model overrides per tap agent `888923e3`
- add event-driven agents, dynamic MCP servers, and comprehensive use case docs `9cbda1aa`
- **mcp**: add stderr capture, server capabilities and tools pagination `d2558a85`
- **cli**: add shell completion for run command and refactor installer `72daf678`
- **webhooks**: add HTTP webhook system for external integrations `04a27df1`
- **mcp**: add session ID support for HTTP MCP servers `b5437843`
- **chat**: add task-aware compression to drain completed user requests `ce29944d`
- **mcp**: split role into domain and spec in session context `449eb46b`
- **compression**: add progressive compression levels for tool-call chains `a6786325`
- **compression**: expand context retention and add analysis findings `eab85ed2`
- **platform**: add Windows named pipe support for cross-platform messaging `8ae10fb1`
- **chat**: add inbox notification banner and improve handling `9a91c367`
- **inbox**: add push_inbox_message_for_session for external tasks `63b53eaf`
- **docs**: add daemon mode documentation for background agents `baea6480`
- **cli**: add daemon mode and external message injection `80afd364`

### 🔧 Improvements & Optimizations

- **webhook_listener**: add comprehensive unit tests `15c58c7c`
- **mcp**: migrate to rmcp SDK for tool handling `069aaf4d`
- **cli**: rename inject command to send `b40fb70a`

### 🐛 Bug Fixes & Stability

- **config**: use octomind.run for OpenRouter referer `dd079efd`
- **mcp**: correct unix-only pgid lazy static `d8cae8a8`
- **mcp**: use PGID to kill busy server processes `e36083a8`
- **session**: resolve cancellation race conditions `6bb53981`
- **mcp**: pass cancellation token to HTTP/stdin tool calls `66d67f09`
- **tests**: prevent ETXTBSY error in webhook listener tests `96b1f9a0`
- **mcp**: replace blocking stdin notification with fire-and-forget method `1485dd73`
- **compression**: replace progressive levels with exponential cooldown `d54b09f0`
- **compression**: preserve last user prompt in tool-loop sessions `2575c408`
- **mcp**: use get_dynamic_server_for_session to retrieve config `58a383d9`
- **mcp**: add reference counting for shared server processes `ee5d8da0`
- **compression**: exclude bootstrap messages from compress_count calculation `848ed9da`
- **session**: enforce session isolation for compression and tools `5bd3b155`
- **run**: suppress tool truncation warnings in non-terminal mode `30a6b6d0`
- **run**: allow daemon mode with terminal input and forward notifications `2eeaa74c`
- **send**: prevent blocking on empty stdin when terminal `5fd594cd`

### 📚 Documentation & Examples

- **readme**: link banner image to website `e747c4d8`
- **config**: document tap agent model overrides `afc1cb4a`
- **readme**: add banner and expand provider list `66357a2b`
- add custom hooks documentation and integration guide `0054704b`
- **use-cases**: add scheduled tasks and long-running development guides `091a74e2`
- **config**: add exponential cooldown and webhook hooks docs `6049f7a2`
- **readme**: refine tagline and messaging `f1604955`
- remove deprecated /cache command references `0ee50d8e`
- **readme**: restructure content and add new agent examples `46b6fa88`

### 🔄 Other Changes

2 maintenance, dependency, and tooling updates not listed individually.

## [0.22.0] - 2026-03-25

### 📋 Release Summary

This release introduces dynamic agent management and multi-session concurrency, allowing AI assistants to run specialized tools and maintain separate contexts across simultaneous sessions. Enhanced MCP server integration brings runtime enable/disable controls, role-based auto-binding, and persistent server management for seamless tool orchestration. Multiple compression and memory improvements deliver more reliable conversation handling, while expanded multimodal support now processes images and videos within prompts.


### ✨ New Features & Enhancements

- **websocket**: process inbox messages as full AI turns before user input `ac36350e`
- **skill**: queue skill content for injection as user message `1d29e795`
- **skill**: add resource catalog builder and comprehensive tests `fed0096d`
- **mcp**: add skill management and discovery system `a0e5f5ae`
- **mcp**: add session-aware dynamic server persistence `fed63cca`
- **session**: add session-scoped context for multi-session concurrency `b2e3fe4a`
- **compression**: add critical knowledge retention configuration `9dad3be1`
- **acp**: add video support to prompt handling `4b132a68`
- **acp**: add image support to prompt handling `fd1abe9a`
- **agent**: add capability resolution for manifests `0b14b4d4`
- **schedule**: inject scheduled messages for all sessions `248457c0`
- **plan**: add forced compression for final task on plan completion `350e570f`
- **mcp**: enhance list command to show configured and dynamic servers `1d8a1817`
- **mcp**: add role-based auto_bind for servers `568e6134`
- **mcp**: add persist/unpersist commands for dynamic servers `10a5dd8c`
- **session**: add HOME placeholder to template system `e4f15409`
- **config**: add default empty roles array to template `69138e5d`
- **website**: reposition as plug-and-play specialist AI agents `dc38ba20`
- **chat**: add force compression option to bypass AI decision `7c9a8bb3`
- **agent**: resolve dependencies before startup `aa5a3661`
- **inputs**: add {{ENV:KEY}} placeholder support with .env fallback `f92589c3`
- **tap**: add local_path argument and auto-inject role names `b201ccc4`
- **dynamic_agents**: allow agents to reference config-defined servers `eceedb60`
- **mcp**: add detailed progress tracking for server initialization `aecc9eea`
- **mcp**: add runtime agent and server enable/disable `f403443c`
- **fs**: return diff output from batch_edit instead of summary `78bb3b24`
- **mcp**: add runtime agent management `e913c4aa`
- **mcp**: add dynamic server tool name resolution `d2029968`
- **mcp**: add runtime server manager `391e1e76`
- **registry**: add tap management for agent manifests `91730e04`
- **run**: add session management options to run command `3200736b`
- **roles**: add optional model override per role `8545b124`
- **prompts**: add {{KEY}} placeholder syntax and agent system `d9bb8a7e`

### 🔧 Improvements & Optimizations

- **commands**: consolidate cache into info and box outputs `e9673067`
- **session**: replace job channels with unified inbox system `a61b93df`
- **session**: move session context setup into session functions `a633c43d`
- **skill**: avoid git pull during skill discovery `1b0f887d`
- **report**: rename human_time to task_time and improve calculation `71f56655`
- **websocket**: extract session lookup and add async job handling `4227fd52`
- **session**: modularize and enhance session management `eb10b1a8`
- **mcp**: split mod.rs and add thread-local workdir support `5aaccb10`
- **mcp**: simplify tool routing logic `5c4c00b0`
- **compression**: switch to token-based recompression threshold `bf360bff`
- **compression**: remove adaptive_threshold flag and enable by default `f0479a4a`
- **config**: reorder capabilities section in default.toml `6a815696`
- **chat**: defer plan compression until project completion `3c192d14`
- **compression**: use actual tokens saved for cooldown calculation `d0ed4300`
- **chat**: remove semantic chunking from compression `612195b9`
- **mcp**: replace builtin filesystem with octofs stdio server `6f77c093`
- **website**: redesign landing page for domain specialists `f33500f2`
- **cli**: consolidate startup flow into single function `4ca27a6e`
- **config**: simplify role configuration `8e9093aa`
- **chat**: improve conversation compression prompt and transcript format `ab5770e1`
- **mcp**: show pending servers instead of completed count `db7b26d9`
- **commands**: improve MCP initialization spinner messages `0b2f7ad5`
- **commands**: unify config and MCP initialization `3a2d425d`
- **tool**: remove ask tool integration `872b84d4`
- **config**: standardize template syntax to {{var}} `9d09d797`
- **cli**: merge session and agent commands into unified run `737c4c77`

### 🐛 Bug Fixes & Stability

- **commands**: remove deprecated /cache command `b789c219`
- **compression**: correct start boundary to preserve first_prompt_idx `e118be2d`
- **input**: handle terminal errors gracefully `c1f4437a`
- **chat**: prevent orphaned summaries and align logger field naming `13f04273`
- **session**: resolve skill injection session context mismatch `ed6c397c`
- **skill**: prevent duplicate skills from being loaded `0d9463ae`
- **tool_execution**: propagate session ID to spawned tool tasks `b79ca36d`
- **run**: enable session-scoped state in CLI mode `5a41f344`
- **compression**: adjust thresholds and update config docs `9ed0db2f`
- **websocket**: make connection handling concurrent and thread-safe `6858f842`
- **compression**: allow tool-loop sessions to compress when no user in preserved zone `c84cb82f`
- **compression**: prevent infinite re-analysis loop on invalid range `9540ed89`
- **logger**: pass session name to log_raw_exchange `2d7d0eef`
- **chat**: ensure first preserved message after compression is user `befb37a7`
- **compression**: enforce hard token ceiling bypassing adaptive logic `ed0693b3`
- **registry**: improve error reporting for failed manifest fetches `2ef8d6c9`
- **config**: replace thread_local role storage with global RwLock `f984e498`
- **inputs**: protect escaped braces before extracting keys `16cf422e`
- **config**: correct octomind acp command syntax `fcf8ae0d`
- **cancellation**: cleanup MCP servers on exit signals `ece81986`
- **animation**: prevent race between spinner cleanup and output `038331e6`
- **mcp**: kill server process group on cancellation `3927b8df`
- **mcp**: prevent deadlock by moving in-flight handles outside process mutex `116d7e5e`
- **chat**: preserve tool results during compression `8480b9b5`
- **chat**: skip cache marker management for non-caching models `0bc24611`
- **chat**: ensure two cache markers after compression `4b3f6c13`
- **chat**: prevent compression of initial user message `0d0a12fd`
- **cancellation**: prevent Ctrl+C deadlock during spinner cleanup `aa844bd6`
- **animation**: prevent Ctrl+C hang on spinner cleanup `e52a3858`
- **config**: deduplicate TOML array tables by name field `590bcc4d`
- **compression**: keep first_prompt_idx anchored to original user message `07934295`
- **compression**: prevent progressive context loss by updating first_prompt_idx `c674873d`
- **animation**: resolve output mode for proper animation display `f3be2ed0`
- **dynamic_agents**: prevent deadlock in clear_all by releasing lock `ad07a662`
- **config**: correct role count assertion in loading test `f62ec767`
- **config**: enable default assistant role configuration `c573b8e3`
- **config**: remove octocode mcp server from default template `2110713a`
- **chat**: handle cancellation as soft signal `78168a13`
- **taps**: add Windows symlink support for tap installation `65ed6940`
- **inputs**: preserve escaped {{{{...}}}} placeholders during substitution `ec4bd8f2`
- **mcp**: correct diff output format and duplicate line detection `bd8b98dc`
- **mcp**: rename stdin to stdio for consistency `9a978a8a`
- **roles**: use full tag as role name instead of suffix `88c165ff`
- **session**: validate provider credentials early `a09b391c`
- **text_editing**: remove has_meaningful_content and use exact line matching `ffdd67a3`
- **mcp**: prefix agent layer names with agent_ `926aad6c`
- **registry**: resolve local agent manifest path lookup `e6bb83f4`

### 📚 Documentation & Examples

- update provider and builtin server documentation `3c3bb2a2`
- add skill tool documentation to core server `7fdbb8db`
- **mcp**: add auto-bind configuration and schedule tool documentation `be003f85`
- **config**: migrate filesystem server to external octofs `e6a31054`
- **readme**: remove asciinema demo and update tap guide link `cb7933c6`
- **readme**: rewrite for specialist agent runtime positioning `5a7a6733`
- update configuration format and add runtime management tools `19b28efa`
- update CLI commands and model configuration `3316dde7`
- update command references from session to run `ac1f17e0`

### 🔄 Other Changes

- **deps**: bump octolib to 0.13.0 and update dependencies `2f53fb98`
- **deps**: bump octolib to 0.12.2 and rand to 0.10 `b3fcd9d8`
- **deps**: bump octolib from 0.12.0 to 0.12.1 `9178a24d`
- **deps**: bump octolib to 0.12.0 and windows-sys to 0.60.2 `dbb7e86b`
- update dependencies `890c7b29`
- **deps**: bump aws-lc-rs and rustls-webpki versions `addc9813`
- **compression**: isolate config loading in tests `04c38b51`
- **docker**: consolidate multi-line docker commands into single lines `6071961b`
- upgrade Rust toolchain to 1.94.0 across workflows `03da8fcc`
- **deps**: bump octolib to 0.10.6 `7be1f87a`
- **deps**: bump octolib to 0.10.5 `750cee71`
- update dependencies `059876ef`
- **dynamic_agents**: serialize tests with mutex to prevent race conditions `989ea301`
- **config**: add developer and assistant test roles `9b9962fe`
- **gitignore**: replace octolib with .marketing directory `5bbcfaa0`

### 📊 Release Summary

**Total commits**: 130 across 5 categories

✨ **33** new features - *Enhanced functionality*
🔧 **26** improvements - *Better performance & code quality*
🐛 **47** bug fixes - *Improved stability*
📚 **9** documentation updates - *Better developer experience*
🔄 **15** other changes - *Maintenance & tooling*

## [0.21.0] - 2026-03-15

### 📋 Release Summary

This release introduces background job management for asynchronous agent execution and real-time result injection while you type, plus a new /jobs command to monitor running tasks. Enhanced sandbox security now restricts file writes to your working directory and safe temporary paths. Multiple bug fixes improve text editing reliability, MCP server stability, and overall system performance.


### ✨ New Features & Enhancements

- **config**: add role validation before session setup `73d2a6c1`
- **session**: add token-aware text truncation function `77e379b2`
- **compression**: add force flag to bypass compression guards `aee1621c`
- **sandbox**: restrict writes to cwd and safe temp paths `d4de75b1`
- **logging**: add structured file tracing for ACP and WebSocket modes `65be2c21`
- **acp**: add slash command support `978eea67`
- **chat**: inject async job results while user is typing `753b6ef2`
- **mcp**: add cancellation for background agent tasks `0602d8e5`
- **chat**: add /jobs command to list background agent jobs `b69c1196`
- **session**: add background job manager for async agents `0e9b6dae`
- **mcp**: add session context to MCP server initialization `2acee52f`
- **compression**: preserve file references in summaries `fe898a92`
- **mcp**: add thread-local working directory for sessions `347d0b25`
- **test**: add ACP smoke test script `5055ef40`
- **acp**: advertise HTTP MCP transport support `324765b4`
- **acp**: add session persistence and MCP server injection `3bde1abf`
- **acp**: add stdio communication protocol support `73aeb16f`

### 🔧 Improvements & Optimizations

- **batch_edit**: simplify editor and add duplicate detection `4eead656`
- **docs**: rename developer server to core `4c810ad6`
- **mcp**: consolidate dev tools into filesystem server `57d5a91e`
- **fs**: remove line count tracking `c856bd44`
- **session**: defer session file creation until first write `073a52c5`
- **done**: replace context reduction with compression `7eb8e77e`
- **mcp**: simplify function descriptions `1f6b1610`
- **mcp**: reorganize servers into core and filesystem categories `fcf809c3`
- **background_jobs**: remove config and use CPU-based limit `3713a10d`
- **mcp**: rename background to async for agent execution `19c2b164`
- **agent**: remove background job management `32598f7e`
- **background_jobs**: replace polling store with push channel `e34ce51e`
- **agent**: remove direct LLM calls from server `1f8dab9e`
- **agents**: migrate to ACP command-based system `31ca5b23`
- **chat**: improve conversation compression prompt structure `20974200`
- **mcp**: unify thread-local working directory handling `7c7cb519`
- **session**: use struct update syntax for GenericSessionArgs `569a859d`

### 🐛 Bug Fixes & Stability

- **config**: switch planner layers to full context input mode `c684a15c`
- **fs**: add double dash separator to grep command `6e5d317b`
- **fs**: block duplicate adjacent lines in line_replace_spec `9f56d155`
- **text_editing**: prevent context start from going below line 1 `b4f10028`
- **text_editing**: block duplicate adjacent lines and require raw text `3efe4c44`
- **sandbox**: correct comment formatting and add missing function `96c95255`
- **logging**: respect output mode suppression in log macros `00e31020`
- **generic**: add MCP response truncation to tool results `1b832f41`
- **mcp**: seed thread-local working directory at startup `0cabeeb5`
- **session**: use thread-local working directory for prompt setup `eaba508d`
- **agent**: ensure cancellation token is available for prompt cancellation `e332e3e9`
- **acp**: ensure MCP servers start before session setup `f0c76305`
- **agent**: allow dynamic MCP server injection via mutable config `36206c68`

### 📚 Documentation & Examples

- **agent**: add background execution parameter documentation `f44ff26b`
- **acp-editor**: add integration documentation `429420cf`

### 🔄 Other Changes

- **release**: add homebrew tap notification job `a4de4d8a`

### 📊 Release Summary

**Total commits**: 50 across 5 categories

✨ **17** new features - *Enhanced functionality*
🔧 **17** improvements - *Better performance & code quality*
🐛 **13** bug fixes - *Improved stability*
📚 **2** documentation updates - *Better developer experience*
🔄 **1** other change - *Maintenance & tooling*

## [0.20.0] - 2026-03-13

### 📋 Release Summary

This release introduces interactive user input tools and enhanced file filtering for more intuitive codebase exploration. The MAP executor now uses streamlined Ollama models with improved worktree isolation and XML-based planning for better task execution. Multiple bug fixes enhance system stability, including improved cancellation handling, better error messages, and resolved build compatibility issues across platforms.


### ✨ New Features & Enhancements

- **plan**: add minimum context threshold for task compression `092337a6`
- **mcp**: add interactive ask tool for user input `fca84305`
- **mcp**: add content-based file filtering to view command `dbe430b6`
- **config**: switch map actors to ollama models `538b717c`
- **mcp**: add thread-local working directory support `f8b366da`
- **config**: add MAP executor configuration template `621274f0`

### 🔧 Improvements & Optimizations

- **config**: remove deprecated continuation and web features `18f6abd5`
- **mcp**: remove web search and consolidate tools `1b927276`
- **commands**: remove ask and shell commands `3f030e0c`
- **chat**: replace continuation with conversation compression `f5582304`
- **ast_grep**: clarify AST pattern syntax and examples `21d0e4ba`
- **map-executor**: replace list_files with view tool `5460729a`
- **fs**: unify file and directory viewing `e581e445`
- **map-planner**: switch to XML subgoals and tighten planning rules `aa93d894`
- **map-executor**: replace branch isolation with worktree isolation `0283858d`
- **config**: simplify planner config and add MAP template `886e06ab`

### 🐛 Bug Fixes & Stability

- **compression**: add cancellation token support to compression checks `5acb79c9`
- **mcp**: prevent hang when previous stdin task times out `017783d6`
- **health_monitor**: add Accept header to HTTP health checks `05680f04`
- **mcp**: add Accept header for JSON and SSE support `42cb21be`
- **brave**: update response field paths for image, video, and news `27804788`
- **spinner**: prevent blocking async runtime during cleanup `8e65c3aa`
- **ask**: return receiver after tool batch completes `33813555`
- **build**: resolve openssl windows compatibility issue `e365f145`
- **build**: resolve musl build failures with vendored openssl `476aa6d2`
- **session**: skip empty instructions files to prevent blank messages `c3796ad6`
- **fs**: improve error message when file exists on create `848ad54e`
- **text_editing**: allow insert at index 0 for file beginning `6f9b060a`
- use config max_retries instead of hardcoded 0 `9cd3f2dd`
- **config**: correct map executor system prompts and formatting `91b3796b`
- **cancellation**: prevent blocking on cancelled spawn tasks `01b59162`
- **websocket**: suppress CLI output in websocket mode `b0e20677`

### 🔄 Other Changes

- **release**: 0.20.0" `58f396f2`
- **deps**: remove openssl-sys dependency `d1ba3cdb`
- **deps**: upgrade clap and related dependencies `a18b39d5`
- **ci**: add musl build job for x86_64 and aarch64 `1f428acf`
- **release**: 0.20.0 `b7d2d43c`
- **release**: add perl dependency for alpine build environment `f943eec0`
- **release**: 0.20.0" `dc331422`
- **deps**: bump octolib to 0.10.3 `1efe161a`
- **release**: 0.20.0 `3db853c9`
- **deps**: bump reedline to 0.46.0 `d3eeadd6`

### 📊 Release Summary

**Total commits**: 42 across 4 categories

✨ **6** new features - *Enhanced functionality*
🔧 **10** improvements - *Better performance & code quality*
🐛 **16** bug fixes - *Improved stability*
🔄 **10** other changes - *Maintenance & tooling*

## [0.19.0] - 2026-03-05

### 📋 Release Summary

This release introduces structured output validation for more reliable AI responses and improves API usage tracking for better session management. Various enhancements include updated dependencies, clearer documentation, and refined shell help organization.


### ✨ New Features & Enhancements

- **session**: add JSON schema validation for output `57e533f8`

### 🔧 Improvements & Optimizations

- **shell**: reorder shell help blocks for clarity `42f15597`

### 🐛 Bug Fixes & Stability

- **tool**: add missing API call increment for follow-up exchanges `395dc98c`

### 📚 Documentation & Examples

- document structured output schema flag `1a0a1300`

### 🔄 Other Changes

- replace manual protoc install with setup-protoc action `885794f7`
- **deps**: bump octolib to 0.10.0 `cfb5c0f8`

### 📊 Release Summary

**Total commits**: 6 across 5 categories

✨ **1** new feature - *Enhanced functionality*
🔧 **1** improvement - *Better performance & code quality*
🐛 **1** bug fix - *Improved stability*
📚 **1** documentation update - *Better developer experience*
🔄 **2** other changes - *Maintenance & tooling*

## [0.18.0] - 2026-02-20

### 📋 Release Summary

This release introduces video attachment support and enhanced CLI flexibility with runtime prompt overrides, plus new WebSocket session handling for real-time collaboration. Compression and animation systems are now smoother and more reliable, with instant cancellation and smarter context management. Numerous bug fixes improve stability across MCP tools, token tracking, and terminal interactions.


### ✨ New Features & Enhancements

- **cli**: add --system and --instructions flags for runtime prompt overrides `89019428`
- **mcp**: add hint accumulator for tool misuse guidance `161946f3`
- **shell**: add hints to guide users toward dedicated MCP tools `6b51a620`
- **websocket**: forward MCP notifications to WebSocket `90c71e1e`
- **websocket**: add client message types and session handling `38d07409`
- **compression**: add adaptive ratio estimation `d38764b0`
- **compression**: add cooldown to prevent premature recompression `6b26a9e5`
- **animation**: add instant Ctrl+C cancellation support `3ada5fc1`
- **chat**: add video attachment support `f64add22`
- **chat**: emit thinking events for non-interactive modes `d23b9798`
- **cache**: separate cache read and write token tracking `efab790c`
- **cli**: add jsonl output to run command `bcdcb4c4`
- **text_editing**: warn on duplicate line_replace calls `452284e1`
- **session**: add reasoning tokens tracking for AI models `459cd2c4`

### 🔧 Improvements & Optimizations

- **compression**: replace heuristic model with physical ceiling math `37936973`
- **animation**: remove redundant cancel_notify assignment `6fd9ae8e`
- **mcp**: replace busy-wait loop with efficient watch channel `c6b35b72`
- **websocket**: replace enum with typed message variants `d970a2d0`
- **websocket**: replace ClientMessage struct with enum variants `3b3b8e21`
- **websocket**: add structured message types `c2cc25ce`
- **plan**: simplify start command to use content parameter `7017b183`
- **commands**: replace console output with structured data `f4f2fbaa`
- **mcp**: remove debug logging from command handlers `98158f5a`
- **context**: remove manual file tree fallbacks to use git only `a8b2d7a2`
- **token_counter**: remove role parameter from token estimation `ac66b9e9`
- **compression**: enhance AI summary prompt structure `14ec2541`
- **animation**: unify animation management and fix zero cost `8c34f9ba`
- **mcp**: clarify method names for readability `f19e528e`
- **animation**: centralize animation control `3101df64`
- **websocket**: replace hack structs with GenericSessionArgs `bfebfca5`
- **chat**: remove unused code and dead fields `65783f69`
- **providers**: remove verbose debug logging for successful thinking conversion `e5c267d9`
- **html_converter**: switch to html2text library `92ceede7`
- **websocket**: replace callback with OutputSink abstraction `4ac59f4f`
- **providers**: remove deepseek provider `34039f62`

### 🐛 Bug Fixes & Stability

- **ask**: prevent custom instructions from affecting ask command `8b653070`
- **cost_tracker**: increment total_api_calls counter `2b84b6fc`
- **mcp**: handle malformed JSON responses gracefully `ffc4a3a0`
- **animation**: update cost before stopping animation `af654d24`
- **chat**: enforce 2-marker limit before inserting compressed block `44e440f1`
- **animation**: prevent leftover stop notification from killing new animation `35d17981`
- **animation**: eliminate busy-polling for instant cancellation `1698d6f9`
- **compression**: prevent marker cache loss during compression `659992f5`
- **fs**: clarify path parameter description for text editor `3eb3377d`
- **tool**: show tool header on error with parameters `3109ecd9`
- **mcp**: improve error messages for list_files directory parameter `c7aad1ff`
- **mcp**: add truncation warnings and apply before size checks `e02dabd8`
- **mcp**: respect zero threshold to disable response warnings `7ec568b0`
- **cli**: handle MCP notifications in both modes `30303e22`
- **providers**: ensure last message is user after compression `0d0fa14a`
- **chat**: prevent clamp panic when api calls is zero `0b646a58`
- **display**: print command results in webserver mode `30e481c4`
- **compression**: verify compression brings context below threshold `95e66058`
- **output**: reexport println macros to prevent spinner ghosting `ff9c0336`
- **animation**: prevent ghost spinner during cost line output `e31fdb3d`
- **session**: preserve multi-turn conversations on Ctrl+C `8e898d33`
- **compression**: respect first user message boundary `e50e2992`
- **animation**: suspend animation during user prompts to prevent interference `8c964aa9`
- **session**: clear stale tool calls when processing restoration markers `31bbe82b`
- **mcp**: stop animation before user prompts `2ba78205`
- **shell**: remove shell history tracking from mcp tool `ab3df70f`
- **chat**: preserve processing state during ctrl+c cleanup `1f01b73a`
- **animation**: prevent premature animation stop in tool loops `89606a39`
- **chat**: prevent ghosted animation by stopping spinner before output `0a30a402`
- **animation**: resolve timing issues with cancellation and spending prompts `0e941538`
- **animation**: prevent duplicate animation start/stop during tool execution `ec4d6020`
- **animation**: remove redundant animation updates `2aad078f`
- **compression**: handle zero-cost models in compression decision `ccfd83f4`
- **animation**: prevent tick condition when spinner finishes `5ccdd5b8`
- **ask**: enable bracketed paste for multiline input `3df7a8fb`
- **tests**: resolve temp dir path on macOS `ee4cd9c6`
- **animation**: update cost and token state after API responses `75f207f5`
- **animation**: eliminate flickering by removing duplicate state updates `47039737`
- **plan**: preserve start index for plan compression `0ed041bc`
- **ast_grep**: handle plain directory paths without glob patterns `26311b21`
- **glob**: handle directory paths in pattern matching `2115e41b`
- **compression**: correct cache cost calculation logic `27dd15b3`
- escape brackets in doc comments `e47badc4`
- **cost**: remove error messages when providers lack cost data `837c7ee6`
- **mcp**: suppress large response warnings in non-terminal modes `124130d2`
- **cli**: rename --mode to --format for clarity `770246f4`
- **output**: suppress plain text logs in jsonl mode `5f631289`
- **token**: correct token counting and jsonl output `d1df2f2a`
- **jsonl**: improve continuation handling and streaming output `399403b0`
- **animation**: resolve terminal race conditions `c1237180`
- **session**: prevent cached stats from overwriting accumulated token counts `cf0f148d`
- **plan**: correct start_index calculation to use valid message index `3420a08a`
- **session**: add truncation point marker for ctrl-c cleanup `acc735da`
- **plan**: resolve index tracking and context loss in compression `adbf75bf`
- **tests**: isolate config loading to prevent race conditions `40ed2998`

### 📚 Documentation & Examples

- **readme**: fix JSONL command flag syntax `ed0e812c`
- rewrite instructions for clarity and brevity `603d7171`
- remove deprecated memory and semantic search features `44b64f56`
- **readme**: rewrite with 3-pillar architecture `e539aa98`
- bump version references to v0.17.0 and update claude model to sonnet-4 `9f9eacc4`
- remove compression improvements documentation `dd1e7dcc`

### 🔄 Other Changes

- **chat**: add missing test attribute to case5 test function `d9eeb7d2`
- **deps**: bump octolib to 0.9.3 and update lockfile `fcd9ce82`
- **plan**: replace title parameter with content field `5d4ddd85`
- **deps**: bump octolib to 0.9.2 `4e50034a`
- **deps**: bump octolib to 0.9.1 `cef9e705`
- **deps**: bump octolib to 0.9.0 and refresh lock file `a5186c0d`
- **deps**: bump octolib to 0.9.0 `aeedc66b`
- **deps**: upgrade octolib to 0.8.3 `f73eb8a4`
- **session**: add session restoration tests `fc3f795f`

### 📊 Release Summary

**Total commits**: 105 across 5 categories

✨ **14** new features - *Enhanced functionality*
🔧 **21** improvements - *Better performance & code quality*
🐛 **55** bug fixes - *Improved stability*
📚 **6** documentation updates - *Better developer experience*
🔄 **9** other changes - *Maintenance & tooling*

## [0.17.0] - 2026-02-12

### 📋 Release Summary

This release introduces intelligent conversation compression that automatically manages context size and costs while preserving important information, plus new keyboard shortcuts and file completion for faster navigation. Enhanced visual feedback includes progress animations, context percentage displays, and helpful tips during sessions. Multiple bug fixes improve token tracking, compression reliability, and overall system stability.


### ✨ New Features & Enhancements

- **config**: add OCTOMIND_CONFIG_PATH environment variable support `cc9a799f`
- **compression**: preserve plan context in compressed task summaries `414e34ff`
- **chat**: detect task completion patterns as critical chunks `f7e52081`
- **compression**: add ignore_cost flag for decision API `337b60d9`
- **compression**: add context-aware bounds for session estimation `827bc57e`
- **compression**: adaptive compression with real pricing `c0c666eb`
- **chat**: add Ctrl+E to exit reverse search `99da1fcc`
- **tool**: display animation during tool execution `79bce9eb`
- **info**: show conversation compression statistics `0824582a`
- **compression**: adaptive pressure-based compression `17573f05`
- **chat**: add animation feedback during compression `0fa57c5c`
- **compression**: add decision model for cost savings `eb45f89b`
- **chat**: add automatic conversation compression `79ba40cf`
- **compression**: add adaptive compression with context pressure detection `f1935080`
- **session**: display randomized tips on new sessions `e93ce76b`
- **edit_mode**: add Meta key bindings for word operations `0798b3a7`
- **input**: add Ctrl+a/Ctrl+e/Ctrl+u navigation shortcuts `7c74d375`
- **chat**: add @ file completion `4942ffb7`
- **chat**: add context percentage display to prompt and animations `b7dcd2e0`
- **modal**: add terminal overlay system for help tooltips `20d6d3a4`
- **config**: add multi-file configuration support `aae24a2b`
- **workflow**: improve execution output with progress tracking `7d7e42bf`
- **commands**: add workflow command, remove layers command `009d85e6`
- **ui**: add progress spinner for MCP server initialization `3d3d7b96`
- **mcp**: add OAuth 2.1 + PKCE support for HTTP MCP `c4a7dd35`

### 🔧 Improvements & Optimizations

- **session**: persist runtime state in SessionInfo `35beae70`
- **chat**: preserve file context in compression `aa123c6f`
- **compression**: use struct for decision model config `c6e1a4e0`
- **compression**: skip compression when no token savings `9e599532`
- **token counting**: rename and improve token estimation `81d7c37d`
- **mcp**: improve edit methods documentation `a178e4b3`
- **compression**: remove pressure_trigger and consolidate threshold logic `afdfdf2d`
- **conversation-compression**: merge decision and summary into single API call `5be9e6d2`
- **diff_chunker**: replace summarization with semantic chunking `c9f3aac3`
- **plan**: simplify compression tracking by removing execution guard `70084746`
- **compression**: implement automatic hierarchical compression system `7a4d3669`
- **session**: auto-process summary requests during continuation `83819969`
- **session**: move tip display function to setup module `6538ab8e`
- **chat**: make queue add action invisible to user `b474074a`
- **chat**: simplify key condition and enable bracketed paste `3390bb46`
- use debug logging for cancellation messages `74acfb57`
- **chat**: migrate to reedline for terminal input `03e8ee86`
- **session/chat**: extract runner into focused modules `ff52e90a`
- **session**: improve interrupt feedback handling `d2088a35`
- **session**: improve exit flow and UI feedback `4b322eaa`
- **workflows**: use iterator pattern for workflow access `36b29ff9`
- **workflows**: improve session and output handling `82c9796d`
- **session**: migrate to workflow architecture `99524dff`
- **text_editor**: improve argument names for clarity `d374541e`
- **chat**: add response_id tracking for OpenAI API `fcd4a89f`

### 🐛 Bug Fixes & Stability

- **plan**: enable automatic continuation after compression in plan `9e2d26a9`
- **session**: prevent token drift and message reload after compression `6cf67d43`
- **compression**: prevent cutting tool messages during compression range calculation `37151054`
- **compression**: reset token counters after compression `84c8ba51`
- **layers**: add missing animation for follow-up API calls `512ef18d`
- **chat**: prevent cutting between assistant tool calls and results `135bfde3`
- **compression**: include first compression message in range calculations `c4a86534`
- **chat**: remove duplicate token tracking in response handlers `d221aedb`
- **chat**: clarify token breakdown labels and cache comment `cf183628`
- **compression**: preserve cache markers during compression `4da7044e`
- **moonshot**: preserve thinking field in tool result processing `98ea2c17`
- **providers**: add error handling for thinking field deserialization `766ad26b`
- **moonshot**: resolve usage tracking and thinking block handling `c30a345b`
- **moonshot**: handle thinking field conversion for reasoning models `7f541520`
- **web**: handle stringified JSON array input to prevent crash `c7b6f478`
- **glob**: normalize paths before pattern matching `e3ad81e4`
- **compression**: correct cache calculation for decision model `dfbf7989`
- **compression**: add debug logging and prevent start_index reset `1cb1b335`
- **compression**: be consistent with effeciency tracking `807d5002`
- **compression**: corruption with missing tools in some cases `cbded7d7`
- **compression**: cache-aware smart adaptive compression logic `ce69f067`
- **tokens**: unify tokens calculation `cfe32edf`
- **compression**: discourse-aware semantic chunking `8355f513`
- **animation**: fix issue with working animation showed before tool execution `c285d150`
- **session/chat**: accurately estimate context with system prompt and tools `c5bb42de`
- **compression**: correct token counting for threshold checks `12867e1f`
- **chat**: add sleep to avoid animation race `e02dbfbf`
- **plan**: adjust start index for tool preservation in compression `dcb9cfc6`
- **plan**: skip compression logic when none pending `953ac2fe`
- **message**: correct inclusive range boundary validation `6eaf6641`
- **input**: add missing $ to cost display indicator `693b9984`
- **session/chat**: handle Ctrl+C during reverse search `9bd34f99`
- **session**: display resume command on Ctrl+D with session ID `47dcae03`
- **session**: show context status line only at session start `6ed1a391`
- **context**: compute accurate context token usage `bf46318b`
- **session**: clean up user message on cancellation during API call `0016e3e1`
- **session**: improve operation tracking for session state `13838bfc`
- **session**: preserve conversation state when API call interrupted after tools `b6831be0`
- **providers**: update response_id field to id for octolib compatibility `db7ee3f0`
- **chat**: skip history for whitespace-prefixed inputs `b133f6b2`
- show correct provider name in cost error messages `52012b22`
- **health_monitor, oauth**: adjust health check interval and fix OAuth discovery `eb6c752d`
- **zai**: resolve provider issue `59523442`

### 📚 Documentation & Examples

- **providers**: update provider format and supported list `c487a93a`
- add compression and multi-file config features `1229e7c0`
- **advanced**: expand future turn estimation with velocity-based analysis `92cee9f3`
- **sessions**: add visual feedback and animation documentation `e825dd04`
- add compression and token system documentation `de0564bd`
- significantly condense INSTRUCTIONS.md developer guide `3f37c65b`
- document config, auth, and workflow features `45bb09ba`
- **installation**: detail shell completion setup for all shells `a531d95a`

### 🔄 Other Changes

- **chat**: extend compression range test for multiple conversation turns `29db9436`
- **deps**: bump octolib to 0.8.0 `31d918ec`
- upgrade Rust toolchain to 1.92.0 `a4979870`
- **ci**: bump Rust toolchain to 1.92.0 `08071883`
- **deps**: update dependency versions in Cargo.lock `b9e6d201`
- upgrade octolib to 0.7.0 `d2bb7896`
- **deps**: upgrade clap to 4.5.56 and rand to 0.9 `74319072`
- **deps**: update octolib to latest version `a66d9ea4`
- **deps**: upgrade cargo dependencies `30a7004c`
- **fs**: update tests to use lines parameter instead of view_range `9ee929a9`
- **deps**: switch local octolib path to version 0.5.0 `5d9b1503`
- **oauth**: fix validation tests for public client scenarios `166b83f8`
- **deps**: bump octolib to 0.4.2 `860e1439`

### 📊 Release Summary

**Total commits**: 114 across 5 categories

✨ **25** new features - *Enhanced functionality*
🔧 **25** improvements - *Better performance & code quality*
🐛 **43** bug fixes - *Improved stability*
📚 **8** documentation updates - *Better developer experience*
🔄 **13** other changes - *Maintenance & tooling*

## [0.16.0] - 2026-01-09

### 📋 Release Summary

This release enhances the conversational experience with thinking headers in messages and structured command outputs (487fe367, b20c975c). Support for zai and minimax AI providers has been added via updated dependencies, alongside improved thinking displays (fae757c1, 1c0add41). Several bug fixes boost file search reliability, cross-platform glob handling including Windows paths, chat consistency, and AI tool integration (ea7ffba4, 5e0405ff, c82d941e, 18dfb4b1, 8fb6d48d, a86e558e, 4fa2360c, dcc7a8ed, c058a6b6).


### ✨ New Features & Enhancements

- **session**: add thinking header to messages `487fe367`
- **server,session**: structured command outputs for websocket server `b20c975c`

### 🔧 Improvements & Optimizations

- **thinking_display**: use fixed separator pattern `1c0add41`
- **display**: centralize list rendering `4eea10c2`
- **session**: simplify message handler `9e52cd26`

### 🐛 Bug Fixes & Stability

- **mcp/fs**: replace map_or with is_some_and for content check `ea7ffba4`
- **fs**: skip content search for empty content `5e0405ff`
- **chat**: prevent duplicate thinking block display `c82d941e`
- **utils**: improve glob pattern handling for Windows paths `18dfb4b1`
- **glob**: handle absolute path patterns without base_dir `8fb6d48d`
- **ast_grep**: fail when globs match no files `a86e558e`
- **session/chat**: warn on empty continuation summary `4fa2360c`
- **file-parser**: prefer <context> tags `dcc7a8ed`
- **gemini**: preserve meta in tool calls `c058a6b6`
- **docker**: update Rust version to `ca1c73ce`

### 📚 Documentation & Examples

- **mcp**: update tool docs and server enhancements `7f48c516`

### 🔄 Other Changes

- **deps**: update octolib to v0.4.0 with zai and minimax providers `fae757c1`
- **deps**: update octolib to 0.3.0 `f4dbc3e7`
- **deps**: bump dependency versions `8f53df17`
- **deps**: update deps and WS API `52478897`
- **workflows**: add missing newline at EOF `a3a70ca5`

### 📊 Release Summary

**Total commits**: 21 across 5 categories

✨ **2** new features - *Enhanced functionality*
🔧 **3** improvements - *Better performance & code quality*
🐛 **10** bug fixes - *Improved stability*
📚 **1** documentation update - *Better developer experience*
🔄 **5** other changes - *Maintenance & tooling*

## [0.15.0] - 2025-11-21

### 📋 Release Summary

This release adds the ability to resume recent sessions and include custom constraints in user inputs, enhancing workflow flexibility (fae183ed, be8c2bd2). It also integrates new AI models, Claude Sonnet 4.5 and GPT-5-Codex, expanding the assistant's capabilities (07a67cc2). Several fixes improve session history migration and command handling, while updates streamline dependencies and provider management for a more reliable experience (7c58af9a, f02967dd, 777bb6d3, 1953a4df).


### ✨ New Features & Enhancements

- **session**: add --resume-recent flag to resume latest session `fae183ed`
- **session**: add support for appending custom constraints to user input `be8c2bd2`
- **models**: add support for Claude Sonnet 4.5 and GPT-5-Codex `07a67cc2`

### 🔧 Improvements & Optimizations

- **providers**: unify providers via octolib and remove legacy code `1953a4df`

### 🐛 Bug Fixes & Stability

- **commands**: remove redundant return statements in ask.rs `7c58af9a`
- **session**: handle legacy history file migration correctly `f02967dd`

### 🔄 Other Changes

- **deps**: remove unused dependencies and clean Cargo.lock `777bb6d3`
- **rust**: update toolchain and dependencies `31ec325e`

### 📊 Release Summary

**Total commits**: 8 across 4 categories

✨ **3** new features - *Enhanced functionality*
🔧 **1** improvement - *Better performance & code quality*
🐛 **2** bug fixes - *Improved stability*
🔄 **2** other changes - *Maintenance & tooling*

## [0.14.0] - 2025-09-16

### 📋 Release Summary

This release adds enhanced command hinting and completion, along with migration support for legacy session history, improving user workflow and continuity (bc3f31b5, 19e4057c). Pricing for DeepSeek has been updated to unified rates effective September 5, 2025 (f40aadca). Additionally, improvements to session history management and clearer error messages enhance overall usability and reliability (3c0afe96, b0d90a0e).


### ✨ New Features & Enhancements

- **session**: add hinting and completion for mor commands (/context, /mcp, /cache, /loglevel, /role, /model) `bc3f31b5`
- **deepseek**: update pricing to unified rates starting Sep 5, 2025 `f40aadca`
- **session**: add migration from legacy global history file `19e4057c`

### 🔧 Improvements & Optimizations

- **history**: implement role-based session history system `3c0afe96`

### 🐛 Bug Fixes & Stability

- **text_editing**: improve str_replace error message guidance `b0d90a0e`

### 📊 Release Summary

**Total commits**: 5 across 3 categories

✨ **3** new features - *Enhanced functionality*
🔧 **1** improvement - *Better performance & code quality*
🐛 **1** bug fix - *Improved stability*

## [0.13.0] - 2025-08-27

### 📋 Release Summary

This release adds new session commands and improved task management features, enhancing user interaction and planning capabilities (/run, /prompt, /plan) (61f22cda, e32c88be, 1be66add). Multimodal support and context handling are improved for smoother workflows (c33d653e, 8b3876de, 36263e31). Several bug fixes address Windows path handling, API cost tracking, and tool response errors, improving overall stability and usability (9a9c656e, 71aa15f1, dd410443). Documentation and installation guides have also been updated for better user guidance (d8426784, 9814b74e).


### ✨ New Features & Enhancements

- **session**: add completion and hints for /run and /prompt `61f22cda`
- **utils**: add context block detection and expansion `c33d653e`
- **chat**: add Ctrl+G to add message without sending `8b3876de`
- **mcp**: require tasks with title and description `e32c88be`
- **plan**: add /plan command to display current plan status `1be66add`
- **truncation**: add global MCP response tokens threshold `36263e31`

### 🔧 Improvements & Optimizations

- **file_parser**: add utilities for file reference extraction and rendering `041b8c46`

### 🐛 Bug Fixes & Stability

- **utils**: preserve Windows drive letters in file path parsing and rendering `9a9c656e`
- **utils**: normalize file paths on Windows in read_file_lines `92543735`
- **prompt**: prevent accidental continuation trigger on /prompt command `848f8293`
- **session**: preserve system message on /run command replace output `d78631c0`
- **session**: track API call cost immediately after response `71aa15f1`
- **cargo**: upgrade dependencies to fix cargo audit issues `6a144ece`
- **fs**: suggest line_replace for ambiguous replacements `3904a5a7`
- **mcp**: handle error flag in MCP tool responses correctly `dd410443`

### 📚 Documentation & Examples

- **config**: update configuration docs with command groups `d8426784`
- **utils**: add file parsing and rendering usage instructions `228c86bc`
- **installation**: rewrite installation guide `9814b74e`
- **plan**: enhance task description guidelines with examples `23b7934b`

### 🔄 Other Changes

- **file_renderer**: fix Windows test failures by normalizing paths and line endings `e57f5aba`
- **plan**: fix task format in plan tests after refactor `b829ef4f`

### 📊 Release Summary

**Total commits**: 21 across 5 categories

✨ **6** new features - *Enhanced functionality*
🔧 **1** improvement - *Better performance & code quality*
🐛 **8** bug fixes - *Improved stability*
📚 **4** documentation updates - *Better developer experience*
🔄 **2** other changes - *Maintenance & tooling*

## [0.12.0] - 2025-08-18

### 📋 Release Summary

This release adds reusable prompt templates and spending tracking to enhance session management, along with batch processing for improved tool efficiency (9848036f, f5b46206, 5a3926f3). Several bug fixes improve search accuracy, prevent retry loops, and enhance system stability (a24932e7, b9d4f783, 4edc057f). Dependency updates and testing refinements further optimize overall performance (a69f72b9, 74d80515).


### ✨ New Features & Enhancements

- **mcp,chat**: batch large output prompts for parallel tool calls `5a3926f3`
- **session**: add /prompt command with reusable templates `9848036f`
- **session**: add request spending threshold and tracking `f5b46206`

### 🐛 Bug Fixes & Stability

- **web**: clarify no-results issue with multiple quoted phrases `a24932e7`
- **style**: update format strings for Clippy compliance `01aaacf0`
- **list_files**: treat content pattern as fixed string to avoid regex errors `efb8a3b5`
- **session**: prevent infinite retry loop on continuation failure `b9d4f783`
- **mcp**: prevent recursion in cancellation polling loop `4edc057f`

### 🔄 Other Changes

- **deps**: update and reorganize dependencies `a69f72b9`
- **fs**: reset line count tracking in batch_edit test `74d80515`

### 📊 Release Summary

**Total commits**: 10 across 3 categories

✨ **3** new features - *Enhanced functionality*
🐛 **5** bug fixes - *Improved stability*
🔄 **2** other changes - *Maintenance & tooling*

## [0.11.0] - 2025-08-10

### 📋 Release Summary

This release adds support for the latest GPT-5 and Anthropic opus-4-1 AI models, expanding the range of AI capabilities (076e797d, b8f3576b). Several improvements enhance session stability and usability, including better input rendering with ANSI color support and cancellation options for agent tools (574d08e5, ed9e2820, e245ecff, a07109dd). Additionally, bug fixes prevent infinite retry loops and improve text editing reliability (574d08e5, 315ddfda).


### ✨ New Features & Enhancements

- **openai**: add GPT-5 model support with pricing and params `076e797d`
- **anthropic**: add opus-4-1 model and fix temp/top_p handling `b8f3576b`

### 🐛 Bug Fixes & Stability

- **session**: prevent infinite retries on continuation calls `574d08e5`
- **text_editing**: prevent repeated line_replace on new lines `315ddfda`
- **mcp**: add cancellation support to agent tool execution `ed9e2820`
- **session**: enable ANSI color mode for rustyline input rendering `e245ecff`
- **session**: prevent infinite retry loops on continuation errors `a07109dd`

### 📊 Release Summary

**Total commits**: 7 across 2 categories

✨ **2** new features - *Enhanced functionality*
🐛 **5** bug fixes - *Improved stability*

## [0.10.2] - 2025-07-26

### 📋 Release Summary

This release improves session reliability by addressing incomplete tool calls, message truncation, and session resuming issues (105fcd80, 2ba67733, a281b8d5, 54aed7c5, 0ace2654, d9ac1b30, 7b5dfada). Additionally, the /done command has been streamlined for better usability, and new quick start and troubleshooting guides have been added to enhance user onboarding (855d1898, 9872b47f).


### 🔧 Improvements & Optimizations

- **done**: move /done command to dedicated file and clean code `855d1898`

### 🐛 Bug Fixes & Stability

- **session**: detect and truncate earliest incomplete tool calls on r... `105fcd80`
- **session**: truncate messages on interrupted tool calls to clean state `2ba67733`
- **session**: correct tool_calls reconstruction on session resume `a281b8d5`
- **session**: restore layers state and cost on session resume `54aed7c5`
- **session**: handle incomplete tool calls in session resuming `0ace2654`
- **session**: re-add initial messages on /done command completion `d9ac1b30`
- **session**: prevent infinite loop on Ctrl+C cancellation `7b5dfada`

### 📚 Documentation & Examples

- **instructions**: add quick start and troubleshooting guide `9872b47f`

### 📊 Release Summary

**Total commits**: 9 across 3 categories

🔧 **1** improvement - *Better performance & code quality*
🐛 **7** bug fixes - *Improved stability*
📚 **1** documentation update - *Better developer experience*

## [0.10.1] - 2025-07-17

### 📋 Release Summary

This release addresses several session management issues to improve stability and user experience, including better handling of cancellation signals and cleanup of partial messages (61a235d4, 959310fb, 7e5ec3ea). Documentation has also been updated to reflect these enhancements.


### 🐛 Bug Fixes & Stability

- **session**: resolve cancellation issue and update documentation `61a235d4`
- **session**: handle cancellation signal correctly in tool loop `959310fb`
- **session**: clean up partial messages on tool and layer cancellation `7e5ec3ea`

### 📊 Release Summary

**Total commits**: 3 across 1 categories

🐛 **3** bug fixes - *Improved stability*

## [0.10.0] - 2025-07-15

### 📋 Release Summary

This release adds customizable output roles for session messages to enhance interaction control (ca1e288f). Several bug fixes improve session stability, including better cancellation handling, prevention of duplicate messages, and stricter configuration enforcement, alongside file lock implementation to avoid write conflicts and improved tool validation (93821f6, 6a9f835e, d25a7cd7, e2a05ccc, d2e707f9, e7277764, 5379398d). Additionally, the session flow has been streamlined for a smoother user experience (4091cf29).


### ✨ New Features & Enhancements

- **session**: add output_role control for session messages `ca1e288f`

### 🔧 Improvements & Optimizations

- **session**: simplify interactive and non-interactive session flow `4091cf29`

### 🐛 Bug Fixes & Stability

- **session**: propagate Ctrl+C cancellation to animation and tools `93821f6c`
- **mcp/fs**: add file locks to prevent concurrent write conflicts `6a9f835e`
- **config**: add missing output_role to default config sections `d25a7cd7`
- **session**: prevent duplicate user messages with layers and continu... `e2a05ccc`
- **session**: remove defaults to enforce strict output role config `d2e707f9`
- **session**: replace ctrlc crate with tokio signal for Ctrl+C handling `e7277764`
- **layers**: apply pattern-based tool validation in layers `5379398d`

### 📊 Release Summary

**Total commits**: 9 across 3 categories

✨ **1** new feature - *Enhanced functionality*
🔧 **1** improvement - *Better performance & code quality*
🐛 **7** bug fixes - *Improved stability*

## [0.9.0] - 2025-07-10

### 📋 Release Summary

This release introduces enhanced session management with dynamic role switching and customizable prompts and temperature settings for improved AI interactions (38ee56f2, 0bdaed2b, 5be4bbc5). Several bug fixes improve configuration stability and reliability, while additional tests ensure robust handling of batch edits (3111ac2f, 20301e74).


### ✨ New Features & Enhancements

- **config**: add top_k and top_p defaults and tune temperatures `38ee56f2`
- **config,ask,shell**: add configurable prompts and temperatures for... `0bdaed2b`
- **session**: add /role command to switch session role at runtime `5be4bbc5`

### 🐛 Bug Fixes & Stability

- **config**: use slice instead of Vec reference in show_mcp_servers `3111ac2f`

### 🔄 Other Changes

- **fs**: add critical batch_edit tests for line number handling `20301e74`

### 📊 Release Summary

**Total commits**: 5 across 3 categories

✨ **3** new features - *Enhanced functionality*
🐛 **1** bug fix - *Improved stability*
🔄 **1** other change - *Maintenance & tooling*

## [0.8.1] - 2025-07-03

### 📋 Release Summary

This release enhances session management with improved task finalization, streamlined continuation handling, and more reliable command processing (5523f0d, cf96448, 6ac8362, bb2f57a). User experience is improved through clearer guidance on parallel tool usage and more intuitive command completion (b328fda, 02e2025). Several bug fixes address token handling, logging clarity, and error reporting to ensure smoother and more stable interactions (5fc894a, fac0836, 2e24515, 58da6f3, ef4eb89).


### 🔧 Improvements & Optimizations

- **session**: simplify continuation logic and unify triggers `feb822c0`
- **session**: remove unused syntax highlighter methods and tests `93727213`

### 🐛 Bug Fixes & Stability

- **session**: update /done command message to "Finalizing current task `5523f0da`
- **session**: remove duplicated token limit log message `5fc894ac`
- **session**: use root max_retries instead of role config `fac08362`
- **session**: respect CLI options over config defaults for runtime pa... `2e245158`
- **session**: simplify continuation logic and avoid user prompt on to... `cf964480`
- **session**: handle continuation immediately to fix tool bug `6ac8362e`
- **session**: stop tool processing on continuation trigger `bb2f57a0`
- **session**: return error on invalid session token threshold at launch `58da6f35`
- **session**: run continuation check before tool calls evaluation `ef4eb893`
- **session**: enable bash-like completion with partial matches and re... `02e20255`

### 📚 Documentation & Examples

- **session**: clarify guidance on parallel tool usage in continuation `b328fdad`

### 📊 Release Summary

**Total commits**: 13 across 3 categories

🔧 **2** improvements - *Better performance & code quality*
🐛 **10** bug fixes - *Improved stability*
📚 **1** documentation update - *Better developer experience*

## [0.8.0] - 2025-07-02

### 📋 Release Summary

This release introduces enhanced session management with improved token tracking, continuation controls, and time monitoring, alongside expanded developer tools and multimodal support (6fabe56, b0ce4657, 3b81c7f). Configurable retry logic and updated pricing models improve AI provider integrations, while comprehensive documentation and testing bolster usability and reliability. Several bug fixes address token calculation accuracy, error handling, and session stability to ensure a smoother user experience (41090fc, c79cfb5f, f7270aee).


### ✨ New Features & Enhancements

- **session**: add role-based token counting and continuation checks `6fabe563`
- **session**: include system prompt and tools in context token count `b0ce4657`
- **session**: add flag to disable continuation triggers temporarily `174ca557`
- **deepseek**: update pricing scheme to use hash maps and helpers `d6ceb6e8`
- **dev**: add plan tool to developer MCP server `3b81c7fd`
- **session**: add API, tool, and total time tracking in layers `d5fcb59b`
- **fs**: support negative line ranges in text editor view_range `63778fed`
- **api**: add configurable retry logic for Amazon provider `2885fb13`
- **session**: preserve initial instructions and welcome messages on ... `880ed2d8`
- **batch_edit**: support single-file multiple operations with origin... `b497a2ca`

### 🔧 Improvements & Optimizations

- **providers**: unify Anthropic and OpenAI retry logic using ret... `ad111d0f`
- **agent,session**: source temperature from role config instead ... `d792da03`
- **batch_edit**: extract batch_edit as independent tool from tex... `2ac3c987`
- **ci**: fix markdown code block formatting in release workflow `0f2fde3e`

### 🐛 Bug Fixes & Stability

- **anthropic**: correct token usage calculation including cache tokens `41090fcd`
- **config**: track and display env var source including .env override `8f3805db`
- **session**: resolve OpenAI 400 errors and add CTRL-C cancellation `c79cfb5f`
- **plan**: prevent overwriting active plan on start command `4bd44337`
- **session**: correct continuation trigger timing in response processing `f513f964`
- **openai**: correct token cost calculation with cache tokens `10716178`
- **openai**: extract and set tool_call_id from response `1d76a461`
- **chat**: use correct model and params for auto threshold continuation `f7270aee`

### 📚 Documentation & Examples

- **mcp**: add detailed docs for new plan tool and usage `f597b7c0`
- **doc**: actualize installation, overview, and config guides `8dc57fce`
- specify cargo check with short message format in instructions `24862099`

### 🔄 Other Changes

- **plan**: fix async test assertions and add serial execution `d44b768a`
- **plan**: add comprehensive tests for plan tool commands `3f49e94a`
- **anthropic**: update model pricing to June 2025 rates `4f558d96`
- **openai**: update model list and pricing to 2025 versions `e9f0841f`

### 📊 Release Summary

**Total commits**: 29 across 5 categories

✨ **10** new features - *Enhanced functionality*
🔧 **4** improvements - *Better performance & code quality*
🐛 **8** bug fixes - *Improved stability*
📚 **3** documentation updates - *Better developer experience*
🔄 **4** other changes - *Maintenance & tooling*

## [Unreleased] - 2025-07-01

### 🐛 Critical Bug Fixes

- **session**: fix OpenAI API 400 errors during parallel tool execution `[current]`
  - Fixed session continuation triggering mid-tool processing causing incomplete tool_calls/tool_results mapping
  - Added `TruncationOptions` struct with `defer_continuation` flag for clean API
  - Implemented proper cancellation support with CTRL-C handling during continuation operations
  - Refactored duplicate functions and eliminated code smells following senior developer practices

### 🔧 Code Quality Improvements

- **context_truncation**: eliminate duplicate functions and parameter pollution `[current]`
  - Replaced `check_and_truncate_context_with_defer` with clean options pattern
  - Removed unused `_role` and `_operation_cancelled` parameters
  - Added proper `TruncationOptions` struct following Rust best practices
  - Implemented cancellation-aware versions with `Option<Arc<AtomicBool>>` pattern

### ✨ New Features

- **cancellation**: add CTRL-C support to session continuation system `[current]`
  - Users can now interrupt long-running summarization operations
  - Added `check_and_truncate_context_with_cancellation` function
  - Added `check_and_handle_continuation_with_cancellation` function
  - Integrated with existing cancellation infrastructure from runner.rs

## [0.7.0] - 2025-06-27

### 📋 Release Summary

This release introduces a new static website for the Octomind project and enhances user experience with improved session continuity and thread-safe history management (d964df21, 7039b455, b06e67b3). Several bug fixes address error handling, input validation, and stability across core features, while dependency updates and documentation improvements further refine overall usability (13e984c4, 52144d2a, 86e45d84, 445db4ca, 049062bd).


### ✨ New Features & Enhancements

- **docs**: add /run command and update continuation architecture docs `e0ff1873`
- **ask**: add separate thread-safe history file for ask mode `b06e67b3`
- **ast_grep**: replace glob crate with ignore for glob expansion `b55fd02f`
- **website**: add complete static site for Octomind project `d964df21`

### 🔧 Improvements & Optimizations

- **fs**: unify error response creation with McpToolResult::error `3dcaf6ed`
- **fs**: make ripgrep line parsing UTF-8 safe `29a74f07`
- **session**: replace manual context limit prompt with auto cont... `564c3423`
- **session**: improve extract_lines output and truncate logic `f2d8c17c`
- **continuation**: extract session summary prompt constants and ... `5abed7d8`

### 🐛 Bug Fixes & Stability

- **text_editing**: add validation and clarify escaped chars error mes... `13e984c4`
- **session**: ensure continuation processes after tool calls complete `7039b455`
- **truncation**: prevent crash by handling char boundaries safely `ab158d51`
- **mcp**: return structured errors for invalid params and cancellations `52144d2a`
- **fs**: return structured errors for invalid parameters and missing ... `86e45d84`
- **commands**: simplify error handling in interactive input `445db4ca`
- **ci**: correct preview URL and remove Lighthouse job `ecda5e8b`
- **install**: correct install script and update master branch path `049062bd`

### 📚 Documentation & Examples

- **fs**: clarify raw content uses actual whitespace, not escapes `3c96628f`
- **session**: update docs to reflect modular continuation refactor `8e53f517`
- **ast_grep**: expand pattern syntax with anonymous wildcard and exa... `8e2d6125`

### 🔄 Other Changes

- **deps**: upgrade html5ever and related crates to latest versions `2ea24fbd`
- **deps**: upgrade multiple dependencies to latest versions `b0be0953`

### 📊 Release Summary

**Total commits**: 22 across 5 categories

✨ **4** new features - *Enhanced functionality*
🔧 **5** improvements - *Better performance & code quality*
🐛 **8** bug fixes - *Improved stability*
📚 **3** documentation updates - *Better developer experience*
🔄 **2** other changes - *Maintenance & tooling*

## [0.6.0] - 2025-06-26

### 📋 Release Summary

This release introduces enhanced session management with smarter context handling and adaptive truncation, improved configuration options including .env key loading, and expanded command capabilities such as line extraction and direct LLM calls (3a99f58, 2fe2d18, 5411078, dc991f2). User experience is improved with clearer cost displays, progress indicators, and robust retry mechanisms (8c52805, fa543c88, f49aef6). Several bug fixes enhance stability, error reporting, and compatibility across sessions, shell commands, and file handling (9ae1a83, af84d01, cf3b0c0).


### ✨ New Features & Enhancements

- **config**: add support for loading keys from .env file `3a99f585`
- **session**: add invisible auto continuation after session reset `2fe2d18d`
- **mcp/fs**: add extract_lines command for line extraction `54110780`
- **session**: add current context tokens to /context output `dc991f26`
- **session**: add adaptive threshold for context truncation `8d2b3414`
- **session**: add smart truncation with internal summarization on to... `8c62e178`
- **session**: add structured summary and file context parsing for co... `c3c593d5`
- **session**: add smart truncation with internal summarization on to... `7d20c893`
- **config**: replace auto-truncation with max session tokens threshold `2a458790`
- **anthropic**: add max-retries rate limiter and enhance retry logging `8c52805e`
- **cli**: add --max-retries flag to session and run commands `a3f2c2e3`
- **ast_grep**: limit glob expansion and improve path handling `9be4366c`
- **ast-grep**: improve output truncation with grouped lines and reus... `e268affa`
- **mcp-shell**: add max_tokens parameter to limit output size `7966360f`
- **chat**: improve cost rendering with intermediate breakdowns and s... `1aef0141`
- **session**: replace custom spinner with indicatif progress bar `f49aef62`
- **chat**: improve cost display with concise static line `fa543c88`
- **mcp**: add call_llm function for direct LLM invocation `4a227a1b`
- **ast_grep**: add glob pattern support for file paths `2c84d591`

### 🔧 Improvements & Optimizations

- **fs**: extract shared file content formatting logic `6e600ef6`
- **mcp**: unify view_file_spec output format for consistency `9788ad3e`
- **session**: unify argument parsing for run commands `c392f325`
- **mcp**: extract MCP initialization into reusable function `acc88534`
- **providers**: unify chat_completion params into struct for cla... `51e882cd`
- **session**: simplify context truncation using summarize method `e3b241cc`
- **fs**: enhance list_files truncation message with stats `b1f6f2ca`

### 🐛 Bug Fixes & Stability

- **session**: correct continuation message processing logic `9ae1a83b`
- **session**: propagate loglevel change to runtime logging macros `3e0cade7`
- **mcp-shell**: follow MCP protocol for shell command responses `af84d019`
- **system-prompts**: ensure all system prompts process templated vari... `90d2d3f0`
- **session**: enforce spending threshold on /run commands `1b797e2f`
- **mcp**: unify response formatting for internal and remote calls `8a830cf3`
- **providers**: log 429 response headers for debug tracing `9656245c`
- **session**: improve spinner color consistency in loading animation `295360fd`
- **agent**: return MCP-compliant errors for invalid inputs and failures `cf3b0c0a`
- **mcp**: return structured errors for invalid ast-grep patterns `ea32caef`
- **session**: preserve tool message order during context truncation `3b46ebaa`
- **session**: lower auto-cache threshold log to debug level `1bcbfea8`
- **commands**: allow optional max_tokens with default fallback `fd238606`
- **session**: preserve and log last assistant summary message after t... `08c3df70`
- **batch_edit**: accept operations as JSON string fallback `463ea146`
- **test**: handle Windows paths in ripgrep output parsing for tests `36f66450`

### 📚 Documentation & Examples

- **shell**: clarify shell command execution and background usage `8cd6f499`
- **session**: improve session continuation prompt clarity and detail `74c628dc`
- **session**: add detailed docs for smart session continuation system `488d4533`
- **fs**: clarify new_str uses raw file content without escaping `46b5aa69`
- **fs**: clarify new_str to avoid escaping and double escaping `e5cc8cb9`
- **fs**: clarify usage guidelines for text editor functions `a281fc64`

### 🔄 Other Changes

- **mcp**: fix shell command tests for MCP response protocol `7a95fd9e`
- **release**: add GitHub Action job to publish crate to crates.io `434487ce`
- **fs**: fix truncation count assertions in fs_tests.rs `13c5b732`
- **fs**: fix file listing test to handle Windows paths `92153860`
- **fs**: fix content search tests for Windows paths" `3d74eccb`
- **fs**: fix content search tests for Windows paths `7a3e8394`

### 📊 Release Summary

**Total commits**: 54 across 5 categories

✨ **19** new features - *Enhanced functionality*
🔧 **7** improvements - *Better performance & code quality*
🐛 **16** bug fixes - *Improved stability*
📚 **6** documentation updates - *Better developer experience*
🔄 **6** other changes - *Maintenance & tooling*

## [0.5.0] - 2025-06-22

### 📋 Release Summary

This release adds support for the DeepSeek AI provider, introduces new session output modes, and enhances code search with the AST-based ast_grep tool. Improvements include more flexible configuration options, detailed cost reporting, and better handling of layered processing contexts. Several bug fixes address file system operations, session stability, tool execution, and server health monitoring to ensure a smoother user experience.


### ✨ Features

- **providers**: add DeepSeek AI provider support (aeb7e707)
- **mcp**: add max_lines and smart truncation to ast_grep output (ab0d498a)
- **session/chat**: improve tool header rendering with context suffix (8157b501)
- **dev**: add ast_grep tool for AST-based code search and refactor (8e4ae7cc)
- **agents**: unify agent configuration with full layer support and a... (056e13c0)
- **session**: add 'last' and 'restart' output modes for layers (5c8a0cf9)
- **cli, config**: add max_tokens option to CLI and config files (49723adf)
- **fs**: add include_hidden option to list_Files function (77f50288)
- **config**: add context_gatherer agent and update workflow instruct... (89553837)
- **session**: display detailed cost breakdown at info log level (9fe01fcc)
- **session**: add sg command version support in helper functions (b4883653)

### 🐛 Bug Fixes

- **ci**: install ripgrep and ast-grep in test workflows (40760793)
- **fs**: correct file listing and content search with ripgrep (def2b93d)
- **fs**: correct ripgrep args and limit list_files output (5cdd1e5e)
- **ast_grep**: pass arguments directly to avoid shell escaping issues (8822e4eb)
- **session**: show tool header for layered processing contexts (f1c896ac)
- **session**: preserve animation during layered response processing (cf00d2b4)
- **agent**: include detailed cost data in agent command results (d62a79ac)
- **session**: resolve rendering issues with layered pipeline output (3773e9ab)
- **session/chat**: correct tool parameter and cost display for agents (956e38d1)
- **fs**: correct line_replace method to replace lines precisely (ae31815e)
- **config**: use root-level max_tokens for all roles (edd6fad8)
- **providers**: remove explicit max_tokens to use default limits (397ee091)
- **layers**: improve Ctrl+C cancellation handling in layers and session (f3031757)
- **layers**: prevent recursive tool calls by checking finish_reason (f773932a)
- **session**: correct layered response user message handling (69b66b4c)
- **config**: remove text_editor tool and developer server ref from MC... (b9c8a8bc)
- **config**: enforce strict layer existence checks for roles (8b3987ff)
- **mcp**: exclude remote HTTP servers on tools/list failure (baa2a82b)
- **animation**: show animation correctly in run command mode (d5dcd16f)
- **session**: correct cache marker placement in parallel tool processing (c11e7659)
- **mcp**: use JSON-RPC POST for HTTP health checks (106e8684)
- **health_monitor**: improve server health checks by type (d7338105)
- **health_monitor**: avoid restarting remote servers as local ones (95f0a5ef)
- **mcp**: improve large output warning with server info and async han... (d95db1e1)
- **session**: enforce session existence on explicit resume (e1870fef)
- **session**: prevent panic by counting chars for content truncation (cf131ee0)
- **mcp**: apply large response threshold consistently in tool calls (b22b22c3)
- **session**: remove assistant messages if any tool results missing (2c3c5d7c)
- **config**: improve developer system prompt clarity and focus (148562b9)
- **tool-execution**: enable immediate cancellation on parallel tool runs (7a8609d5)
- **session/chat**: correct tool header rendering in parallel execution (748f5d63)

### 🔧 Other Changes

- **docker**: add ast-grep and octocode tools to runtime image (f71a3e1a)
- **ast_grep**: group output by file to reduce token usage (38917779)
- **instructions**: clarify development restrictions and code quality... (3f5eac5f)
- add guidelines for efficient Rust development builds (db80bcde)
- **agent**: simplify output merge logic for append mode (4bf2e000)
- clarify code search tools and update usage guidelines (066cbdfa)
- **fs**: add extensive async tests for str_replace function (1d7f81a9)
- **config**: remove hardcoded octocode references from MCP servers (8646c1b7)
- **config**: remove invalid fallback test for unknown role (b97085af)
- **providers**: remove explicit max_tokens to use default limits" (df71e134)
- **logging**: replace eprintln with octomind logging macros (04709e81)
- **layers**: improve layered output formatting and readability (507c4c2f)
- **config**: rename query_processor and context_generator layers... (cc273a9a)
- **config**: enforce explicit system prompt and strict role config (3a4c9563)
- **config**: improve default developer role config and usage guidance (5fd74c50)
- **INSTRUCTIONS**: tune md file with proper guidanance (cc1852b6)
- **tool_map**: derive Default and simplify function pointers (a6d98eb2)
- **mcp**: use static tool map for tool-to-server routing (1168b388)
- **config**: simplify and clarify developer prompt instructions (1ada9b9f)
- **config**: improve mcp.servers config parsing and templates (c05d1fff)
- **config**: revise default developer system prompt for clarity and ... (3b340567)
- **mcp**: split web module into api_client and formatters files (8f1b9fc3)
- **session**: improve MCP tool call rendering for single and mul... (0bdf99f9)

### 📊 Commit Summary

**Total commits**: 65
- ✨ 11 new features
- 🐛 31 bug fixes
- 🔧 23 other changes

## [0.4.0] - 2025-06-17

### 📋 Release Summary

This release introduces new web search tools, including image, video, and news search, along with Brave Search integration and a built-in web server for enhanced functionality. User experience is improved with support for input from stdin, streamlined single-task session handling, and clearer messaging during layered processing. Several bug fixes enhance session stability, output handling, and recursive tool usage.


### ✨ Features

- **config**: add allowed tools filtering from config patterns (0a2add63)
- **web**: add image, video, and news search tools with docs (a98a9abc)
- **config**: add builtin web server with web tools support (662a4dc6)
- **websearch**: add Brave Search integration and web MCP server (a6541e4d)
- **run**: support input from stdin if no parameter provided (46d36043)
- **session**: add user message for automatic layered processing (b5760314)
- **run**: add run command for single-task session handling (0fc4894e)

### 🐛 Bug Fixes

- **session**: disable animation in non-interactive run sessions (864d4206)
- **session**: handle multiple outputs correctly in layered processing (dd5eff99)
- **session**: prevent duplicate user message addition in layered resp... (50b1fc6f)
- **session/layers**: enable recursive tool calls in layers using unif... (256498bf)

### 🔧 Other Changes

- **instructions**: add detailed guidance on where to look first (92fa9661)
- **web-mcp**: move html2md functionality into read_html tool (94b798a7)

### 📊 Commit Summary

**Total commits**: 13
- ✨ 7 new features
- 🐛 4 bug fixes
- 🔧 2 other changes

## [0.3.0] - 2025-06-16

### 📋 Release Summary

This release introduces customizable chat sessions with support for custom instruction files, role-based welcome messages, and enhanced command output handling. Several improvements streamline session context management and configuration options. Multiple bug fixes enhance stability by addressing error handling, session state preservation, and server process isolation.


### ✨ Features

- **session**: add support for custom instructions file in chat sessions (f90c6e61)
- **config**: add role-based welcome messages and %{ROLE} variable (3af97d99)
- **session**: add output_mode handling for command results (8cb4fc57)
- **session**: add filtering to display session context command (2e17ac7e)

### 🐛 Bug Fixes

- **mcp**: return compliant error on user decline of large output (e304ac02)
- **mcp**: isolate server processes to ignore Ctrl+C termination (3da2fce7)
- **session**: remove broken assistant message on empty tool results (a07250ee)
- **session**: remove user message on API call failure to prevent poll... (dab04601)
- **session**: preserve conversation state after tool execution interr... (2b18c71a)

### 🔧 Other Changes

- **docker**: add .dockerignore to exclude unnecessary files (9958f82e)
- **cargo**: remove unused dependencies from Cargo.lock (0001157b)
- **deps**: upgrade multiple dependencies to latest versions (2fa4bfe1)
- **cli**: use dynamic version from Cargo.toml in CLI (3b6c902f)
- **config**: add custom instructions file feature documentation (445e3421)
- **instructions**: add detailed AI project guide and config principles (53d7b004)
- **session**: move context reduction logging after message update (1084f94d)
- **mcp**: replace server_type with type and remove mode field (c6305838)
- **config**: remove octocode availability check and builtin flags (f7a7aeee)
- **commands**: move reduce command from layers to commands defin... (640b4831)
- **layers**: simplify layers and remove unused configs (a40ecca4)
- **changelog**: reformat changelog entries for consistency (d777f1d5)

### 📊 Commit Summary

**Total commits**: 21
- ✨ 4 new features
- 🐛 5 bug fixes
- 🔧 12 other changes

All notable changes to this project will be documented in this file.

## [0.2.0] - 2025-06-14

### 📋 Release Summary

This release enhances session management with new commands like /reduce, /context, dump, and validate for improved user control and feedback, including detailed responses for unknown commands. Tool support is expanded to Amazon and Cloudflare providers, while session stability is improved through better handling of cancellations and tool call preservation. Additional refinements include configurable AI agents for task routing, enhanced prompts, and updated documentation for clearer guidance.


### ✨ Features

- **session**: add detailed feedback for unknown commands (44994ead)
- **session/display**: add token count and percentage per message (1059a5ae)
- **session**: add /reduce command to compress session history (b5aa8047)
- **config**: enhance query_processor and context_generator prompts (fe3bbf41)
- **session**: add tool support to Amazon and Cloudflare providers (b6488700)
- **session**: add /context command to display session context (809d3929)
- **fs**: enhance line replacement feedback with detailed snippet and... (6b0cf942)
- **agent**: add configurable AI agents for task routing (42c7cb45)
- **config**: add parsing support for custom roles in config (4f2f1b6e)
- **session**: add dump and validate commands for MCP tools (47c61946)

### 🐛 Bug Fixes

- **session**: update debug toggle command in display message (33019763)
- **mcp**: preserve server process on cancellation (e7b7923c)
- **session**: clean up tool_calls on Ctrl-C cancellation (1462e056)
- **session/list**: add markdown rendering with plain text fallback (8276cba9)
- **session**: ensure tool_calls match results after tool execution (9f4f0e22)
- **session**: clean up incomplete tool_calls on interrupt (a7286a9e)
- **session**: preserve valid tool requests on Ctrl+C interruption (79b6c475)
- **session**: reset full session context on Ctrl+C cancellation (98fbae08)
- **commands**: disable MCP tools for ask and shell commands (8a1e6f7b)
- **session**: sort tool functions to ensure consistent order (d55915e4)
- **session**: remove /debug command and make /loglevel runtime-only (0ef1594d)
- **session**: safely truncate strings by counting chars instead of bytes (3bcc67d5)
- **config**: enforce explicit temperature in role configs (fb335b25)
- **session**: ensure immediate cancellation on Ctrl+C during follow-up (d678183c)
- **session**: preserve complete tool sequences during truncation (a411d4e2)

### 🔧 Other Changes

- **fs**: reduce prompt tokens in MCP function definitions (29b0f28b)
- **providers**: move providers out of session module (1a34c663)
- **session**: split chat commands into separate files (e8ffcd80)
- **fs**: enhance text editor command usage guidance and examples (ab184809)
- **config**: document layered architecture with named layers (b9fc0dbd)
- add asciinema demo to README (a4cd5fb5)
- **config**: update config file location to system-wide directory (605b9c89)
- **fs**: clarify text_editing tool definitions and usage warnings (01d57dbd)
- **config**: rename mode to role across codebase (c96dc3da)
- **session**: unify tool-to-server lookup for /mcp command (b3678a52)
- **config**: rename get_mode_config to get_role_config consistently (dcbb882c)
- add Cargo.lock to repository tracking (243dc8ab)
- **config**: clarify agent configs and update examples (517e58ec)
- **chat**: unify tool execution for sessions and layers (7ed9af58)
- **mcp**: add MCP result helpers and improve undo output (50647017)
- **mcp**: add tool-to-server map for routing tool calls (9dcb710a)
- **config**: unify role configs using roles array format (208b7251)
- **deps**: upgrade multiple dependencies and add new crates (ceeece54)

### 📝 All Commits

- 33019763 fix(session): update debug toggle command in display message *by Don Hardman*
- e7b7923c fix(mcp): preserve server process on cancellation *by Don Hardman*
- 1462e056 fix(session): clean up tool_calls on Ctrl-C cancellation *by Don Hardman*
- 8276cba9 fix(session/list): add markdown rendering with plain text fallback *by Don Hardman*
- 9f4f0e22 fix(session): ensure tool_calls match results after tool execution *by Don Hardman*
- 44994ead feat(session): add detailed feedback for unknown commands *by Don Hardman*
- 29b0f28b refactor(fs): reduce prompt tokens in MCP function definitions *by Don Hardman*
- 1059a5ae feat(session/display): add token count and percentage per message *by Don Hardman*
- a7286a9e fix(session): clean up incomplete tool_calls on interrupt *by Don Hardman*
- 1a34c663 refactor(providers): move providers out of session module *by Don Hardman*
- e8ffcd80 refactor(session): split chat commands into separate files *by Don Hardman*
- b5aa8047 feat(session): add /reduce command to compress session history *by Don Hardman*
- 79b6c475 fix(session): preserve valid tool requests on Ctrl+C interruption *by Don Hardman*
- fe3bbf41 feat(config): enhance query_processor and context_generator prompts *by Don Hardman*
- 98fbae08 fix(session): reset full session context on Ctrl+C cancellation *by Don Hardman*
- ab184809 docs(fs): enhance text editor command usage guidance and examples *by Don Hardman*
- 8a1e6f7b fix(commands): disable MCP tools for ask and shell commands *by Don Hardman*
- b9fc0dbd docs(config): document layered architecture with named layers *by Don Hardman*
- a4cd5fb5 docs: add asciinema demo to README *by Don Hardman*
- 605b9c89 docs(config): update config file location to system-wide directory *by Don Hardman*
- b6488700 feat(session): add tool support to Amazon and Cloudflare providers *by Don Hardman*
- d55915e4 fix(session): sort tool functions to ensure consistent order *by Don Hardman*
- 0ef1594d fix(session): remove /debug command and make /loglevel runtime-only *by Don Hardman*
- 809d3929 feat(session): add /context command to display session context *by Don Hardman*
- 01d57dbd docs(fs): clarify text_editing tool definitions and usage warnings *by Don Hardman*
- 6b0cf942 feat(fs): enhance line replacement feedback with detailed snippet and... *by Don Hardman*
- c96dc3da refactor(config): rename mode to role across codebase *by Don Hardman*
- b3678a52 refactor(session): unify tool-to-server lookup for /mcp command *by Don Hardman*
- dcbb882c refactor(config): rename get_mode_config to get_role_config consistently *by Don Hardman*
- 243dc8ab chore: add Cargo.lock to repository tracking *by Don Hardman*
- 517e58ec docs(config): clarify agent configs and update examples *by Don Hardman*
- 3bcc67d5 fix(session): safely truncate strings by counting chars instead of bytes *by Don Hardman*
- 7ed9af58 refactor(chat): unify tool execution for sessions and layers *by Don Hardman*
- 42c7cb45 feat(agent): add configurable AI agents for task routing *by Don Hardman*
- fb335b25 fix(config): enforce explicit temperature in role configs *by Don Hardman*
- d678183c fix(session): ensure immediate cancellation on Ctrl+C during follow-up *by Don Hardman*
- 50647017 refactor(mcp): add MCP result helpers and improve undo output *by Don Hardman*
- 9dcb710a refactor(mcp): add tool-to-server map for routing tool calls *by Don Hardman*
- 208b7251 refactor(config): unify role configs using roles array format *by Don Hardman*
- 4f2f1b6e feat(config): add parsing support for custom roles in config *by Don Hardman*
- 47c61946 feat(session): add dump and validate commands for MCP tools *by Don Hardman*
- ceeece54 chore(deps): upgrade multiple dependencies and add new crates *by Don Hardman*
- a411d4e2 fix(session): preserve complete tool sequences during truncation *by Don Hardman*

## [0.1.0] - 2025-06-10

## Your AI Development Companion is Here!

We're excited to announce the first official release of **Octomind** - an AI-powered development assistant that transforms how you interact with your codebase through natural conversations.

## 🎯 What Makes This Release Special

**Session-First Development** - No more complex CLI commands or setup. Just start a conversation with AI and get things done. Whether you're debugging, refactoring, or exploring new code, Octomind understands your project context and helps you work smarter.

**Multi-Provider AI Support** - Choose from OpenRouter, OpenAI, Anthropic, Google, Amazon, or Cloudflare. Switch between models on the fly and find the perfect AI assistant for your specific task.

**Built-in Development Tools** - File operations, code analysis, shell commands, and more - all accessible through natural conversation. No need to leave your AI session to get work done.

## ✨ Key Features in v0.1.0

- 🤖 **Interactive AI Sessions** with intelligent context management
- 🛠️ **Integrated Development Tools** via MCP protocol
- 🌐 **6 AI Provider Integrations** with unified interface
- 🖼️ **Multimodal Vision Support** - analyze images, screenshots, and diagrams
- 💰 **Real-time Cost Tracking** with detailed usage reports
- 🔧 **Role-Based Configuration** - Developer and Assistant modes
- 📊 **Smart Caching System** for cost optimization
