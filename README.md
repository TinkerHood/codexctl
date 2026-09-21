# Codex Controller (`codexctl`)

[![CI](https://github.com/TinkerHood/codexctl/actions/workflows/ci.yml/badge.svg)](https://github.com/TinkerHood/codexctl/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.94%2B-blue.svg)](https://www.rust-lang.org)

> **Codex Controller** - Full control plane for Codex CLI

**Prerequisite**: Requires [@openai/codex](https://www.npmjs.com/package/@openai/codex) to be installed first.

**Version**: 0.10.1 | **Author**: [Bhanu Korthiwada](https://github.com/BhanuKorthiwada)

🔗 **Website**: [codexctl on GitHub](https://github.com/TinkerHood/codexctl)
📖 **Documentation**: See README for usage

---

## Why Codex Controller?

Codex Controller starts from a practical limitation in Codex CLI today: there is no native first-class multi-profile workflow. This repo starts with a full end-to-end control plane for profile management, usage visibility, switching, and concurrent terminal usage.

Use it when you need to:

- 🔐 **Multi-account management** - Switch between work/personal/dev accounts instantly
- 🤖 **Automation** - Run Codex in CI/CD with specific credentials
- 📊 **Usage monitoring** - Track quota across teams
- 🌳 **Concurrent sessions** - Use multiple accounts in parallel
- 🔄 **Profile-based workflows** - Environment-specific configurations

- 🔐 **Securely store** multiple Codex CLI profiles with optional encryption
- ⚡ **Switch instantly** between accounts without re-authenticating
- 🤖 **Auto-switch** based on quota availability
- 📊 **Monitor usage** across all your Codex accounts
- 🌳 **Select saved accounts** - run a command with a chosen profile

---

## Codex vs API (Important)

- ChatGPT/Codex plans and the OpenAI API are separate products with separate billing.
- A ChatGPT plan does not automatically include API usage credits.
- Codex CLI can authenticate with either a ChatGPT account or an API key.
- `codexctl usage` reads ChatGPT/Codex plan claims from local auth tokens.
- `codexctl usage --realtime` checks API billing/quota via OpenAI API endpoints.
- `codexctl status --json`, `usage --json`, `verify --json`, and `doctor --json` provide script-safe structured output.
- References:
  - Codex CLI auth flow: https://developers.openai.com/codex/cli
  - API vs ChatGPT billing separation: https://help.openai.com/en/articles/8156019

---

## Features

### Multi-Account Management
- 🔐 **Secure Profiles** - Store multiple Codex credentials with optional encryption
- ⚡ **Instant Switching** - Switch accounts in < 1 second
- 🔄 **Quick Toggle** - Toggle between current and previous with `codexctl load -`
- 🗂️ **Full CRUD** - Save, load, list, delete, backup profiles

### Automation & Control
- 🤖 **Auto-Switcher** - Automatically pick best profile based on quota
- 📊 **Usage Monitoring** - Profile claims and optional legacy API billing estimates
- ✅ **Verify** - Validate all profiles' authentication status
- 🌳 **Concurrent Sessions** - Use multiple accounts in parallel
- 🏃 **CI/CD Integration** - Run with specific credentials in pipelines

### Developer Experience
- 🖥️ **Cross-Platform** - macOS, Linux, Windows (WSL2)
- 🔧 **Shell Completions** - Bash, Zsh, Fish, PowerShell
- 🧪 **Zero Runtime** - Single binary, no Node.js required
- 🐳 **Docker** - Multi-arch images
- 📦 **Import/Export** - Transfer profiles between machines

---

## Quick Start

### Prerequisites

First, install Codex CLI:

```bash
# Install Codex CLI (required)
pnpm add -g @openai/codex

# Verify installation
codex --version
```

### Install `codexctl`

```bash
# Via cargo
cargo install codexctl

# Or via npm
pnpm add -g codexctl

# Or download binary from GitHub Releases
curl -fsSL https://github.com/TinkerHood/codexctl/releases

```

### First Steps

```bash
# Save your current Codex CLI profile
codexctl save work

# Create another profile
# (switch accounts in Codex CLI, then:)
codexctl save personal

# List all profiles
codexctl list

# Switch to a profile
codexctl load work

# Quick-switch to previous profile
codexctl load -
```

---

## Commands

```
codexctl save <name>              Save current Codex auth as a profile
codexctl load <name>              Load a saved profile and switch to it
codexctl list                     List all saved profiles
codexctl delete <name>            Delete a saved profile
codexctl status                   Show current profile status
codexctl usage                    Show plan claims and API quota context
codexctl verify                   Verify all profiles' authentication status
codexctl backup                   Create a backup of current profile
codexctl run --profile <name> -- <cmd>
                                  Run a command with a specific profile
codexctl env <name>               Export profile selection variables
codexctl diff <name1> <name2>     Compare/diff two profiles
codexctl switch                   Switch to a profile interactively (fzf)
codexctl history                  View command history
codexctl doctor                   Run health check on profiles
codexctl completions              Generate shell completions
codexctl import <name> <b64>      Import a profile from another machine
codexctl export <name>            Export a profile for transfer
codexctl setup                    Interactive setup wizard
```

---

## Encryption

```bash
# Save with encryption
codexctl save work --passphrase "my-secret"

# Or use environment variable
export CODEXCTL_PASSPHRASE="my-secret"
codexctl save work

# Load encrypted profile
codexctl load work --passphrase "my-secret"

# Run one command with an encrypted profile, then restore original auth
codexctl run --profile work --passphrase "my-secret" -- codex --version
```

---

## Usage And Auto-Switching

The optional `--realtime` view uses legacy OpenAI billing endpoints and is an
unverified estimate, not authoritative account quota. Failed or malformed usage
responses are reported as errors. `load auto` ranks saved plan claims; it does
not measure current remaining quota.

Inspect usage directly or let the controller pick the best available profile:

```bash
# Show current profile usage details
codexctl usage

# Emit structured JSON for automation
codexctl usage --json

# Compare usage across all saved profiles
codexctl usage --all

# Select a profile using saved plan claims
codexctl load auto
```

---

## Profile Environment And One-Shot Commands

Print profile environment variables or run a command with temporary authentication:

```bash
# Print shell exports for a profile
codexctl env work

# Bash/Zsh example
eval "$(codexctl env work)"

# Run one command against a specific profile and restore after
codexctl run --profile work -- codex --version
```

`codexctl env` emits codexctl-specific variables; it does not provide isolated Codex sessions. `load` and `run` use the shared Codex auth file, so use one active identity at a time.

`codexctl load` and `codexctl run` only swap the live `auth.json`. Existing local sessions, history, memories, and state stay untouched. The automatic backup created during `load` now captures the live `auth.json` only, matching the actual mutation surface.

---

## Structured Output

For CI, shell pipelines, and editor tooling:

```bash
codexctl status --json
codexctl usage --json
codexctl usage --all --json
codexctl verify --json
codexctl doctor --json
```

---

## Shell Completions

```bash
source <(codexctl completions bash --print)
```

---

## Docker

```bash
# Run with Docker
docker run -it --rm \
  -v ~/.codexctl:/home/codexctl/.local/share/codexctl \
  -v ~/.codex:/home/codexctl/.codex \
  ghcr.io/tinkerhood/codexctl list
```

---

## Configuration

Profiles are stored in the platform's application data directory by default.
Use `codexctl status` to inspect the resolved paths, or choose a profile directory:

```bash
codexctl --config-dir /path/to/profiles list
# Equivalent environment override:
export CODEXCTL_DIR=/path/to/profiles
```

`CODEXCTL_PASSPHRASE` supplies the encryption passphrase and `CODEXCTL_QUIET`
controls quiet output. There is no `codexctl` configuration-file parser.

Profile names cannot use internal names (`backups`, dot-prefixed names) or
command aliases (`auto`, `-`). Save and import prepare replacements before
replacing an existing profile. Named backups refuse an existing destination;
automatic backups receive unique names. Exports are written under the private `.exports` directory in the profiles
directory as `.exports/<name>.export.txt`; prior export files are excluded from archives.

`codexctl run` restores auth after the child exits, propagates its exit code, and
reports restoration failures even in quiet mode. Auth changes are guarded by a
process lock. After a forced process stop, the next codexctl invocation recovers
the saved auth when it is safe to do so; unrelated external auth changes are
retained and reported for manual recovery. Profile replacement also retains a
recoverable original across process interruption. These locks coordinate
codexctl processes, not external Codex clients. Keep recovery files until any
reported conflict is resolved.

On Unix, `run` stops its command process group before restoring auth, including
shell descendants, and passes foreground terminal input to interactive commands.
Commands that deliberately detach into another process group are outside this
cleanup. Windows signal handling stops the immediate child only. Killing codexctl
with SIGKILL cannot run cleanup immediately; stop any surviving command before
invoking codexctl to recover the saved auth.

---

## Contributing

We welcome contributions! See [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines.

## Security

For responsible disclosure and supported-version policy, see [SECURITY.md](./SECURITY.md).

## Support

For usage help and reporting guidance, see [SUPPORT.md](./SUPPORT.md).

## Changelog

Release history is tracked in [CHANGELOG.md](./CHANGELOG.md).

## License

MIT License - see [LICENSE](./LICENSE) for details.

---

Built by [Bhanu Korthiwada](https://github.com/BhanuKorthiwada)
