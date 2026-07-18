# `ticket` fuzz targets

Fuzz harnesses for the `ticket` contract, built on [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
(libFuzzer). They run outside the normal `cargo test` flow: locally on demand,
and on a schedule in CI (`.github/workflows/fuzz.yml`) rather than on every PR,
since each run needs tens of seconds to be useful and a crash doesn't block
merging — it opens an issue instead.

## Requirements

- A **nightly** Rust toolchain: `rustup toolchain install nightly`. `cargo-fuzz`
  builds with `-Z sanitizer=address`, which only exists on nightly.
- `cargo install cargo-fuzz` (one-time).

## Running a target locally

From `ticket/fuzz/`:

```bash
cargo +nightly fuzz run fuzz_mint                        # run until interrupted
cargo +nightly fuzz run fuzz_mint -- -max_total_time=60   # run for 60s and stop
```

`cargo +nightly fuzz list` prints all target names.

## Targets and invariants

| Target | Invariant checked |
|---|---|
| `fuzz_mint` | `initialize`/`mint_ticket` never Rust-panic for arbitrary `max_tickets`/`price`/`max_resale` inputs — only typed Soroban contract errors are allowed. |
| `fuzz_transfer` | The resale-price cap holds exactly at the boundary: `transfer_ticket` must succeed for `sale_price <= max_resale` and fail for `sale_price > max_resale`, for any `i128` price. |
| `fuzz_verify` | `verify_ticket` errors (never panics) on a nonexistent ticket id, returns `true` and flips status to `Used` on first verify of a valid ticket, returns `false` on a second verify of an already-`Used` ticket, and always errors for a caller who isn't the organizer. |
| `fuzz_full_lifecycle` | Across randomly chosen mint → transfer → verify paths (including transferring a `Used` ticket, which must fail) the contract never Rust-panics and each operation's result matches the expected state transition. |

A "Rust panic" here means an unhandled `panic!`/`unwrap`/arithmetic overflow
escaping the contract; expected `Result::Err`s from `try_*` client calls are not
failures and are asserted on explicitly where the invariant requires it.

## Regression seeds

Any input that reproduces a crash is committed under `corpus/<target>/` (one
file per input, named by its content hash — the default `cargo fuzz` crash
naming). `cargo fuzz run` automatically loads `corpus/<target>/` as seed input
before generating new cases, so committed crashes are replayed on every future
run and can never regress silently.

`corpus/` and `artifacts/` are gitignored as a whole (the exploration corpus is
large and reproducible, not something to track wholesale), so capturing a
regression seed requires force-adding the specific file:

```bash
cargo +nightly fuzz run <target>                 # crashes, writes to artifacts/<target>/crash-<hash>
cp artifacts/<target>/crash-<hash> corpus/<target>/
git add -f corpus/<target>/crash-<hash>
```
