# Wallet Radar agent guide

This file is the shared contract for Codex, Claude, and other coding agents.
Read it before changing code. More focused Rust rules live in `.cursor/rules/`.

## Architecture that must not drift

- Production is Rust: `rust-server/` is the HTTP/WS backend; `backfill/`,
  `nextaction/`, `features/`, `patterns/`, and `gat/` are analytics/offline crates;
  `satellite/` is the key-holding gRPC proxy; `wr-store/` owns SQLite reference data.
- `server/` is legacy Node kept as a behavioral oracle for `parity_check`. It is NOT in the
  public repository, so `parity_check`, `npm start`, and the `tools/gen_*_fixtures.mjs`
  generators only work where that folder exists. Golden tests that need
  `rust-server/tests/fixtures/` skip (print `SKIP:`) when the fixtures are absent. Never
  make it the production source of truth.
- The core must not receive provider API keys. Keyed provider calls go through
  `satellite/` over mTLS, and providers must remain allowlisted to prevent SSRF.
- Wallets, exchange addresses, thresholds, groups, and strategy parameters belong
  in `wr-store` migrations/data, not hard-coded Rust constants.
- `proto/satellite.proto` is the gRPC source of truth. Never hand-edit generated code.
- Preserve response shape and byte-level behavior where golden/parity tests exist.

## How to work

1. Inspect the affected crate, neighboring modules, tests, and parity fixture before editing.
2. State the domain types and invariants before implementing non-trivial behavior.
3. Make the smallest coherent change; do not opportunistically rewrite legacy areas.
4. Keep `main`/bin files as wiring. Put parsing, validation, and computation in testable functions.
5. Add or update tests in the same change. A bug fix starts with a reproducing test.
6. Run checks for every affected crate; there is no root Cargo workspace.

## Rust design baseline

- Make illegal states unrepresentable when an invariant is meaningful. Use a newtype
  to constrain data, not merely to rename a primitive and immediately unwrap it.
- Parse untrusted strings at the boundary into typed values (`Url`, enums, validated
  addresses/IDs). Prefer exhaustive enum matches over stringly typed branching.
- Use the narrowest ownership contract: `&str`/`AsRef<str>` to read, `Into<String>`
  when storing, and `Cow` only when borrow-or-own is genuinely useful.
- Avoid unnecessary clones and allocations. Validate borrowed input before allocating.
- Add only trait bounds actually used. Prefer static dispatch unless runtime
  heterogeneity or decoupling requires `dyn Trait`.
- Libraries expose typed `thiserror` errors; binaries may add `anyhow::Context`.
  Preserve source chains. Use `Result` when callers need to know why an operation failed.
- Do not add `unwrap`, `expect`, or `panic!` to request handling, workers, parsers, or
  libraries. They are acceptable in tests and for proven invariants with an explanatory comment.
- Do not introduce `unsafe` without explicit approval and a `// SAFETY:` invariant.

## Async, data, and observability

- Do not block Tokio executors; use async I/O or `spawn_blocking` for blocking/CPU work.
- Never hold a `std::sync` lock across `.await`; keep lock scopes short and audit lock order.
- Worker errors need an explicit retry/skip/fail policy; do not silently break loops.
- Use structured `tracing`, not `println!`, in runtime code. Never log secrets or credentials.
- Use UTC-aware time. Keep deterministic analytics deterministic; seed randomness in tests.
- Use transactions for multi-write invariants and parameterized SQL only.

## Required validation

For each changed Rust crate, run from that crate directory:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Also run the relevant repository checks:

```bash
npm test                 # golden/parser parity
npm run build            # TypeScript + browser assets, when web changes
```

Run `parity_check` when changing an API route or behavior covered by the Node oracle.
Do not claim completion if a required check was skipped; state why and what remains.

## Review checklist

- Are new states/invariants enforced once at construction or repeatedly at call sites?
- Would a new enum variant or struct field force all affected code to be revisited?
- Are error paths, malformed input, empty data, duplicates, and boundary values tested?
- Are clones, allocations, broad trait bounds, and lock scopes justified?
- Could the change leak a key, bypass the satellite, permit arbitrary hosts, or expose
  internal errors to clients?
- Does the change preserve golden output, API shape, SQLite ownership, and Node/Rust parity?
- Is the diff free of secrets, generated junk, IDE files, and unrelated formatting churn?

