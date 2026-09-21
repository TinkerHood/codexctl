# Codex Controller (`codexctl`)

Codex Controller for Codex CLI. The first end-to-end slice is profile management, switching, usage visibility, and concurrent terminal workflows.

## Installation

```bash
pnpm add -g codexctl
```

Or run without a global install:
```bash
pnpm dlx codexctl --help
```

## Quick Start

```bash
# Save your current CLI profile
codexctl save work
codexctl save personal

# Switch between profiles
codexctl load work

# List all profiles
codexctl list
```

## Features

- Optional encryption for sensitive auth data
- Fast profile switching
- Usage visibility across saved Codex profiles
- Export to use profiles concurrently in different terminals

## Binary Package

This package uses platform-specific optional dependencies containing pre-built
binaries. Keep optional dependencies enabled when installing it.

Supported platforms:
- Linux (x86_64, arm64)
- macOS (x86_64, arm64)  
- Windows (x86_64)

## License

MIT
