#!/usr/bin/env bash
#
# Octraban contracts — build, MVP-lower, and deploy to Stellar testnet.
#
# Background:
#   These contracts pin soroban-sdk 21, whose on-chain VM rejects the WebAssembly
#   `reference-types` and `multivalue` features. Modern Rust (>= 1.82) bakes those
#   features into every wasm it produces (including std), and the `-C target-feature`
#   flag does NOT reliably strip them. The reliable fix is to build normally, then
#   lower the wasm to the exact feature set Soroban accepts using Binaryen's wasm-opt.
#
# Prereqiuisites:
#   - rustup with a wasm target:   rustup target add wasm32-unknown-unknown
#   - Stellar CLI:                 https://github.com/stellar/stellar-cli
#   - Binaryen (wasm-opt):         https://github.com/WebAssembly/binaryen/releases
#   - A funded testnet identity:   stellar keys generate <name> --network testnet --fund
#
# Usage:
#   ./build-and-deploy.sh [IDENTITY] [NETWORK]
#     IDENTITY  Stellar CLI key alias to deploy from   (default: octraban-deployer)
#     NETWORK   target network                          (default: testnet)
#
set -euo pipefail

IDENTITY="${1:-octraban-deployer}"
NETWORK="${2:-testnet}"

# Feature set accepted by the soroban-sdk 21 VM: MVP + bulk-memory + sign-ext +
# mutable-globals, with reference-types and multivalue explicitly removed.
WASM_OPT_FLAGS=(
  --disable-reference-types
  --disable-multivalue
  --enable-bulk-memory
  --enable-bulk-memory-opt
  --enable-sign-ext
  --enable-mutable-globals
  -Oz
)

command -v wasm-opt >/dev/null || { echo "❌ wasm-opt (Binaryen) not found on PATH"; exit 1; }
command -v stellar  >/dev/null || { echo "❌ stellar CLI not found on PATH"; exit 1; }

# crate dir : produced wasm file name (as emitted by cargo)
declare -A CRATES=(
  [explorer]="octraban_contract.wasm"
  [ticket]="ticket.wasm"
)

mkdir -p dist
echo "Identity: ${IDENTITY}   Network: ${NETWORK}"
echo

for crate in "${!CRATES[@]}"; do
  raw="${CRATES[$crate]}"
  echo "━━━ ${crate} ━━━"
  ( cd "${crate}" && cargo build --release --target wasm32-unknown-unknown )
  src="${crate}/target/wasm32-unknown-unknown/release/${raw}"

  out="dist/${crate}.wasm"
  echo "  lowering to MVP-compatible feature set → ${out}"
  wasm-opt "${src}" -o "${out}" "${WASM_OPT_FLAGS[@]}"

  echo "  deploying…"
  id="$(stellar contract deploy --wasm "${out}" --source "${IDENTITY}" --network "${NETWORK}")"
  echo "  ✅ ${crate} contract id: ${id}"
  echo
done

echo "Done. Record the contract IDs above in DEPLOYMENTS.md."
