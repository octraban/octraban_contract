#!/usr/bin/env bash
set -euo pipefail

# Opt-in on-chain deployment verification for Octraban testnet.
#
# Usage:
#   ./scripts/verify-deployment.sh --run
#   ./scripts/verify-deployment.sh --run \
#     --expected-explorer-wasm-hash <HEX> \
#     --expected-ticket-wasm-hash <HEX>
#
# This script is intentionally NOT wired into `cargo test`.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPLOYMENTS_FILE="${ROOT_DIR}/DEPLOYMENTS.md"

RUN=0
EXPECTED_EXPLORER_WASM_HASH=""
EXPECTED_TICKET_WASM_HASH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run)
      RUN=1
      shift
      ;;
    --expected-explorer-wasm-hash)
      EXPECTED_EXPLORER_WASM_HASH="$2"
      shift 2
      ;;
    --expected-ticket-wasm-hash)
      EXPECTED_TICKET_WASM_HASH="$2"
      shift 2
      ;;
    -h|--help)
      cat <<EOF
verify-deployment.sh (opt-in)

Checks basic read-only behavior of deployed testnet contracts.

Examples:
  ./scripts/verify-deployment.sh --run
  ./scripts/verify-deployment.sh --run \
    --expected-explorer-wasm-hash <hex> \
    --expected-ticket-wasm-hash <hex>
EOF
      exit 0
      ;;
    *)
      echo "Unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

if [[ "$RUN" != "1" ]]; then
  echo "Opt-in required. Re-run with: --run" >&2
  exit 3
fi

if ! command -v stellar >/dev/null 2>&1; then
  echo "❌ 'stellar' CLI not found on PATH" >&2
  exit 1
fi

if [[ ! -f "$DEPLOYMENTS_FILE" ]]; then
  echo "❌ DEPLOYMENTS.md not found at $DEPLOYMENTS_FILE" >&2
  exit 1
fi

RPC_URL=""
NETWORK_PASSPHRASE=""
EXPLORER_ID=""
TICKET_ID=""

# Extract relevant bits from DEPLOYMENTS.md.
RPC_URL="$(python - <<PY
import re
p=open(r"$DEPLOYMENTS_FILE","r",encoding="utf-8").read()
m=re.search(r"^RPC:\s*`([^`]*)`",p,flags=re.M)
print(m.group(1).strip() if m else "")
PY
)"
NETWORK_PASSPHRASE="$(python - <<PY
import re
p=open(r"$DEPLOYMENTS_FILE","r",encoding="utf-8").read()
m=re.search(r"Network passphrase:\s*`([^`]*)`",p,flags=re.M)
print(m.group(1).strip() if m else "")
PY
)"
EXPLORER_ID="$(python - <<PY
import re
p=open(r"$DEPLOYMENTS_FILE","r",encoding="utf-8").read()
m=re.search(r"\|\s*Explorer\s*\|[^\|]*\|\s*`([A-Z0-9]{56})`\s*\|",p)
print(m.group(1) if m else "")
PY
)"
TICKET_ID="$(python - <<PY
import re
p=open(r"$DEPLOYMENTS_FILE","r",encoding="utf-8").read()
m=re.search(r"\|\s*Ticket\s*\|[^\|]*\|\s*`([A-Z0-9]{56})`\s*\|",p)
print(m.group(1) if m else "")
PY
)"

if [[ -z "$RPC_URL" || -z "$NETWORK_PASSPHRASE" || -z "$EXPLORER_ID" || -z "$TICKET_ID" ]]; then
  echo "❌ Failed parsing DEPLOYMENTS.md" >&2
  exit 1
fi

echo "Using RPC: $RPC_URL"
echo "Explorer contract id: $EXPLORER_ID"
echo "Ticket contract id:   $TICKET_ID"

# Many view-only Soroban methods still accept a dummy source address for
# simulation. We use an all-zero (valid-looking) public key as a placeholder.
DUMMY_SOURCE="GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"

# Read-only simulation helper. If your installed stellar-cli does not support
# simulating view calls via `contract invoke`, this will fail fast.
call_view() {
  local cid="$1"
  local fn="$2"

  # We keep fee low and timeout bounded. No auth is expected for view methods.
  stellar contract invoke \
    --network testnet \
    --source "$DUMMY_SOURCE" \
    --id "$cid" \
    --function "$fn" \
    --rpc-url "$RPC_URL" \
    --fee "100" \
    --timeout "30s"
}

# Explorer: is_paused()
echo "\n[1/3] Explorer: is_paused()"
set +e
out1=$(call_view "$EXPLORER_ID" "is_paused" 2>&1)
rc=$?
set -e
if [[ $rc -ne 0 ]]; then
  echo "❌ Failed calling explorer.is_paused via stellar-cli." >&2
  echo "$out1" >&2
  exit $rc
fi

echo "$out1"

# Explorer: event_count()
echo "\n[2/3] Explorer: event_count()"
set +e
out2=$(call_view "$EXPLORER_ID" "event_count" 2>&1)
rc=$?
set -e
if [[ $rc -ne 0 ]]; then
  echo "❌ Failed calling explorer.event_count via stellar-cli." >&2
  echo "$out2" >&2
  exit $rc
fi

echo "$out2"

# Ticket: tickets_sold()
echo "\n[3/3] Ticket: tickets_sold()"
set +e
out3=$(call_view "$TICKET_ID" "tickets_sold" 2>&1)
rc=$?
set -e
if [[ $rc -ne 0 ]]; then
  echo "❌ Failed calling ticket.tickets_sold via stellar-cli." >&2
  echo "$out3" >&2
  exit $rc
fi

echo "$out3"

# Optional wasm hash drift checks.
# NOTE: This is best-effort and depends on stellar-cli output formats.
if [[ -n "$EXPECTED_EXPLORER_WASM_HASH" || -n "$EXPECTED_TICKET_WASM_HASH" ]]; then
  echo "\n[optional] wasm hash checks requested"
  echo "Note: depends on stellar-cli 'contract inspect' output."

  if [[ -n "$EXPECTED_EXPLORER_WASM_HASH" ]]; then
    echo "- explorer wasm hash"
    insp=$(stellar contract inspect --network testnet --rpc-url "$RPC_URL" --id "$EXPLORER_ID" 2>&1 || true)
    echo "$insp"
    if echo "$insp" | grep -qi "$EXPECTED_EXPLORER_WASM_HASH"; then
      echo "  ✅ matched expected explorer wasm hash"
    else
      echo "  ⚠️ expected explorer wasm hash not found" >&2
      exit 4
    fi
  fi

  if [[ -n "$EXPECTED_TICKET_WASM_HASH" ]]; then
    echo "- ticket wasm hash"
    insp=$(stellar contract inspect --network testnet --rpc-url "$RPC_URL" --id "$TICKET_ID" 2>&1 || true)
    echo "$insp"
    if echo "$insp" | grep -qi "$EXPECTED_TICKET_WASM_HASH"; then
      echo "  ✅ matched expected ticket wasm hash"
    else
      echo "  ⚠️ expected ticket wasm hash not found" >&2
      exit 4
    fi
  fi
fi

echo "\n✅ Deployed testnet contracts respond as expected for read-only endpoints."

