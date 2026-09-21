# Contributing

Thanks for contributing to Codex Controller (`codexctl`).

## Development Setup

Prerequisites:

- Rust stable (`rustup`)
- Node.js 20+ (for npm wrapper updates)

Clone and validate locally:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features --all-targets
pnpm --dir npm exec node --test test/launcher.test.js
cargo build --release --locked
```

`./pre-commit` runs formatting, lint, Rust tests, and launcher tests without
rewriting files. Install `cargo-audit` and `cargo-machete` for dependency checks:
`cargo audit --deny warnings` and `cargo machete`.

## Contribution Guidelines

- Keep changes scoped and focused.
- Add or update tests for behavior changes.
- Update docs for user-facing changes.
- Keep command examples consistent with the current CLI surface.

## Commit And PR Expectations

- Use clear commit messages.
- Describe:
  - what changed
  - why it changed
  - how it was validated

Before opening a PR:

1. Rebase onto latest `main`.
2. Run format, lint, and test checks locally.
3. Ensure no secrets or credentials are committed.

## Security Contributions

For security-sensitive issues, use private disclosure:
https://github.com/repohelper/codexctl/security/advisories/new
