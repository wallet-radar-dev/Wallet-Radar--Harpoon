# Security Policy — Wallet Radar

## Reporting a Vulnerability
Please report security issues privately to the maintainer. Do not open
public issues for security matters. We aim to acknowledge within 72 hours.

## Data Handling & Privacy
- **Public data only.** Wallet Radar reads exclusively PUBLIC on-chain
  (blockchain) data. It never requests, stores, or transmits private
  keys, seed phrases, or custody of any funds.
- **No PII collection.** Beyond the operator's own watch-list of wallet
  addresses, no personal data is collected.
- **Local-first storage.** Data is stored locally (JSON files / Docker
  volume) under the operator's control.
- **Not financial advice.** All output is analytical/research information
  only.

## Access Control
- **Two-tier authentication** (HTTP Basic):
  - `ADMIN_PASSWORD` — full access (owner).
  - `APP_PASSWORD` — read-only demo (viewers cannot modify/delete data).
- **Brute-force protection** — per-IP lockout after repeated failures.
- **Read-only demo mode** — mutating endpoints return 403 for viewers.

## Application Security Controls
- OWASP secure response headers (X-Content-Type-Options, X-Frame-Options,
  Referrer-Policy, Permissions-Policy, HSTS, COOP).
- `x-powered-by` disabled (no stack disclosure).
- Input validation on all address/contract parameters (strict regex).
- Rate limiting on heavy analytical endpoints.
- Global crash guards (uncaughtException / unhandledRejection) — a single
  bad request cannot take down the service.
- Graceful error handler — no stack traces leaked to clients.
- Automated daily backups with retention.

## Secrets
- Credentials and API keys live only in `.env` (never committed / never
  hardcoded in source).

## Testing
- Automated test suite (`npm test`) covering classification, portfolio
  construction, and financial math.
- Strict TypeScript (`strict`, `noUnusedLocals`) enforced at build.
