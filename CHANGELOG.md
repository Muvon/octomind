# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2025-06-14

### 📋 Release Summary

This release enhances session management with new commands like /reduce, /context, dump, and validate for improved user control and feedback, including detailed responses for unknown commands. Tool support is expanded to Amazon and Cloudflare providers, while session stability is improved through better handling of cancellations and tool call preservation. Additional refinements include configurable AI agents for task routing, enhanced prompts, and updated documentation for clearer guidance.


### ✨ Features

- **session**: add detailed feedback for unknown commands ([`44994ead`])
- **session/display**: add token count and percentage per message ([`1059a5ae`])
- **session**: add /reduce command to compress session history ([`b5aa8047`])
- **config**: enhance query_processor and context_generator prompts ([`fe3bbf41`])
- **session**: add tool support to Amazon and Cloudflare providers ([`b6488700`])
- **session**: add /context command to display session context ([`809d3929`])
- **fs**: enhance line replacement feedback with detailed snippet and... ([`6b0cf942`])
- **agent**: add configurable AI agents for task routing ([`42c7cb45`])
- **config**: add parsing support for custom roles in config ([`4f2f1b6e`])
- **session**: add dump and validate commands for MCP tools ([`47c61946`])

### 🐛 Bug Fixes

- **session**: update debug toggle command in display message ([`33019763`])
- **mcp**: preserve server process on cancellation ([`e7b7923c`])
- **session**: clean up tool_calls on Ctrl-C cancellation ([`1462e056`])
- **session/list**: add markdown rendering with plain text fallback ([`8276cba9`])
- **session**: ensure tool_calls match results after tool execution ([`9f4f0e22`])
- **session**: clean up incomplete tool_calls on interrupt ([`a7286a9e`])
- **session**: preserve valid tool requests on Ctrl+C interruption ([`79b6c475`])
- **session**: reset full session context on Ctrl+C cancellation ([`98fbae08`])
- **commands**: disable MCP tools for ask and shell commands ([`8a1e6f7b`])
- **session**: sort tool functions to ensure consistent order ([`d55915e4`])
- **session**: remove /debug command and make /loglevel runtime-only ([`0ef1594d`])
- **session**: safely truncate strings by counting chars instead of bytes ([`3bcc67d5`])
- **config**: enforce explicit temperature in role configs ([`fb335b25`])
- **session**: ensure immediate cancellation on Ctrl+C during follow-up ([`d678183c`])
- **session**: preserve complete tool sequences during truncation ([`a411d4e2`])

### 🔧 Other Changes

- **fs**: reduce prompt tokens in MCP function definitions ([`29b0f28b`])
- **providers**: move providers out of session module ([`1a34c663`])
- **session**: split chat commands into separate files ([`e8ffcd80`])
- **fs**: enhance text editor command usage guidance and examples ([`ab184809`])
- **config**: document layered architecture with named layers ([`b9fc0dbd`])
- add asciinema demo to README ([`a4cd5fb5`])
- **config**: update config file location to system-wide directory ([`605b9c89`])
- **fs**: clarify text_editing tool definitions and usage warnings ([`01d57dbd`])
- **config**: rename mode to role across codebase ([`c96dc3da`])
- **session**: unify tool-to-server lookup for /mcp command ([`b3678a52`])
- **config**: rename get_mode_config to get_role_config consistently ([`dcbb882c`])
- add Cargo.lock to repository tracking ([`243dc8ab`])
- **config**: clarify agent configs and update examples ([`517e58ec`])
- **chat**: unify tool execution for sessions and layers ([`7ed9af58`])
- **mcp**: add MCP result helpers and improve undo output ([`50647017`])
- **mcp**: add tool-to-server map for routing tool calls ([`9dcb710a`])
- **config**: unify role configs using roles array format ([`208b7251`])
- **deps**: upgrade multiple dependencies and add new crates ([`ceeece54`])

### 📝 All Commits

- [`33019763`] fix(session): update debug toggle command in display message *by Don Hardman*
- [`e7b7923c`] fix(mcp): preserve server process on cancellation *by Don Hardman*
- [`1462e056`] fix(session): clean up tool_calls on Ctrl-C cancellation *by Don Hardman*
- [`8276cba9`] fix(session/list): add markdown rendering with plain text fallback *by Don Hardman*
- [`9f4f0e22`] fix(session): ensure tool_calls match results after tool execution *by Don Hardman*
- [`44994ead`] feat(session): add detailed feedback for unknown commands *by Don Hardman*
- [`29b0f28b`] refactor(fs): reduce prompt tokens in MCP function definitions *by Don Hardman*
- [`1059a5ae`] feat(session/display): add token count and percentage per message *by Don Hardman*
- [`a7286a9e`] fix(session): clean up incomplete tool_calls on interrupt *by Don Hardman*
- [`1a34c663`] refactor(providers): move providers out of session module *by Don Hardman*
- [`e8ffcd80`] refactor(session): split chat commands into separate files *by Don Hardman*
- [`b5aa8047`] feat(session): add /reduce command to compress session history *by Don Hardman*
- [`79b6c475`] fix(session): preserve valid tool requests on Ctrl+C interruption *by Don Hardman*
- [`fe3bbf41`] feat(config): enhance query_processor and context_generator prompts *by Don Hardman*
- [`98fbae08`] fix(session): reset full session context on Ctrl+C cancellation *by Don Hardman*
- [`ab184809`] docs(fs): enhance text editor command usage guidance and examples *by Don Hardman*
- [`8a1e6f7b`] fix(commands): disable MCP tools for ask and shell commands *by Don Hardman*
- [`b9fc0dbd`] docs(config): document layered architecture with named layers *by Don Hardman*
- [`a4cd5fb5`] docs: add asciinema demo to README *by Don Hardman*
- [`605b9c89`] docs(config): update config file location to system-wide directory *by Don Hardman*
- [`b6488700`] feat(session): add tool support to Amazon and Cloudflare providers *by Don Hardman*
- [`d55915e4`] fix(session): sort tool functions to ensure consistent order *by Don Hardman*
- [`0ef1594d`] fix(session): remove /debug command and make /loglevel runtime-only *by Don Hardman*
- [`809d3929`] feat(session): add /context command to display session context *by Don Hardman*
- [`01d57dbd`] docs(fs): clarify text_editing tool definitions and usage warnings *by Don Hardman*
- [`6b0cf942`] feat(fs): enhance line replacement feedback with detailed snippet and... *by Don Hardman*
- [`c96dc3da`] refactor(config): rename mode to role across codebase *by Don Hardman*
- [`b3678a52`] refactor(session): unify tool-to-server lookup for /mcp command *by Don Hardman*
- [`dcbb882c`] refactor(config): rename get_mode_config to get_role_config consistently *by Don Hardman*
- [`243dc8ab`] chore: add Cargo.lock to repository tracking *by Don Hardman*
- [`517e58ec`] docs(config): clarify agent configs and update examples *by Don Hardman*
- [`3bcc67d5`] fix(session): safely truncate strings by counting chars instead of bytes *by Don Hardman*
- [`7ed9af58`] refactor(chat): unify tool execution for sessions and layers *by Don Hardman*
- [`42c7cb45`] feat(agent): add configurable AI agents for task routing *by Don Hardman*
- [`fb335b25`] fix(config): enforce explicit temperature in role configs *by Don Hardman*
- [`d678183c`] fix(session): ensure immediate cancellation on Ctrl+C during follow-up *by Don Hardman*
- [`50647017`] refactor(mcp): add MCP result helpers and improve undo output *by Don Hardman*
- [`9dcb710a`] refactor(mcp): add tool-to-server map for routing tool calls *by Don Hardman*
- [`208b7251`] refactor(config): unify role configs using roles array format *by Don Hardman*
- [`4f2f1b6e`] feat(config): add parsing support for custom roles in config *by Don Hardman*
- [`47c61946`] feat(session): add dump and validate commands for MCP tools *by Don Hardman*
- [`ceeece54`] chore(deps): upgrade multiple dependencies and add new crates *by Don Hardman*
- [`a411d4e2`] fix(session): preserve complete tool sequences during truncation *by Don Hardman*
