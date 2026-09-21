# Security Policy

## Supported Versions

Security fixes are currently prioritized for the latest `main` branch and the most recent release tag.

| Version | Supported |
|---|---|
| Latest release (`v0.6.x`) | Yes |
| Older releases | Best effort |

## Reporting A Vulnerability

Please do not open public issues for security reports.

1. Open a private security advisory in GitHub:
   https://github.com/TinkerHood/codexctl/security/advisories/new
2. Include:
   - affected version
   - reproduction steps
   - impact assessment
   - suggested fix (if available)

## Response Targets

- Initial triage: within 72 hours
- Confirmation and severity assessment: within 7 days
- Patch or mitigation plan: as soon as practical based on severity

## Security Notes

- This project stores authentication artifacts and supports optional encrypted profile storage.
- We run dependency and static security scans in CI.
- Upstream advisory warnings with no available fix are documented and re-evaluated on dependency updates.
- Current temporary RustSec ignore:
  - `RUSTSEC-2026-0097` (`rand 0.8.5` via `age-core`), tracked in `.cargo/audit.toml` until upstream releases a patched dependency chain.
