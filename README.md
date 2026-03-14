# Octomind 🤖 - AI-Powered Development Assistant

**© 2026 Muvon Un Limited** | [Documentation](doc/README.md)

> **Session-based AI coding assistant** with **extensible architecture**, **smart codebase understanding**, and **zero AI provider lock-in**

[![asciicast](https://asciinema.org/a/wpZmOSOgFXp8HRzTltncgN7e3.svg)](https://asciinema.org/a/wpZmOSOgFXp8HRzTltncgN7e3)

---

## Why Octomind?

| Problem | Octomind Solution |
|---------|-------------------|
| Limited tool extensibility | ⚙️ Configure any agent as MCP via config |
| One-size-fits-all AI | 🧠 Plan-first with custom layers |
| Locked into one provider | 🌐 7 providers, switch instantly |

---

## ✨ The 3 Pillars

### ⚙️ Fully Extensible
- **Configure any agent as MCP tool** — just add to config
- **Custom commands** with prompt templates
- Full control over execution flow

### 🧠 Smart Context + Plan-First
- **[Octocode](https://github.com/muvon/octocode)** — Semantic code search, knowledge graph, GraphRAG
- **[Octobrain](https://github.com/muvon/octobrain)** — Persistent memory, knowledge base, memory graphs
- **Plan-first** — Multi-step planning with validation gates

### 🌐 Provider Freedom
- **7 providers**: OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek
- **Switch instantly**: `/model openai:gpt-4o` or `/model deepseek:v3`
- Know your costs — Real-time tracking included

---

## 🚀 Quick Start

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Set API key
export OPENROUTER_API_KEY="your_key"

# Start session
octomind run
```

---

## 💬 How It Works

```
> "How does auth work in this project?"
[AI searches codebase, explains implementation]

> "Add rate limiting to login"
[AI edits code, shows changes]

> "Plan the refactor of the auth module"
[AI creates multi-step plan with validation]
```

### Built-in Tools

- **Shell**: Execute commands
- **Editor**: Edit files, batch changes
- **Search**: ast_grep
- **Web**: Search, read HTML

---

## 🔌 Run Modes

| Mode | Command | Use For |
|------|---------|------|
| Interactive | `octomind run` | Daily development |
| Registry agent | `octomind run developer:rust` | Specialised agent from registry |
| WebSocket | `octomind server --port 8080` | Automation, IDE plugins |
| JSONL | `echo "task" \| octomind run --format jsonl` | CI/CD pipelines |

---

## 🛠️ Installation

```bash
# One-line install
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Or build from source
git clone https://github.com/muvon/octomind.git
cd octomind
cargo build --release

# Set your API key
export OPENROUTER_API_KEY="sk-or-v1-..."
```

**Supported providers:** OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek

---

## 📖 Documentation

- [Installation](doc/01-installation.md)
- [Configuration](doc/03-configuration.md)
- [Providers](doc/04-providers.md)
- [Sessions](doc/05-sessions.md)
- [Layers & Workflows](doc/07-command-layers.md)

---

## Company

**Muvon Un Limited** (Hong Kong) | [Website](https://muvon.io) | [Issues](https://github.com/muvon/octomind/issues)

**License:** Apache 2.0
