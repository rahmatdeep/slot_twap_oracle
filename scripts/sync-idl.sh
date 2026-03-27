#!/usr/bin/env bash
#
# Copies the generated IDL JSON from target/ into sdk/src/idl.json,
# then verifies the SDK still compiles.
#
# Usage:
#   anchor build && bash scripts/sync-idl.sh
#
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/target/idl/slot_twap_oracle.json"
DEST="$ROOT/sdk/src/idl.json"

if [ ! -f "$SRC" ]; then
  echo "Error: $SRC not found. Run 'anchor build' first."
  exit 1
fi

cp "$SRC" "$DEST"
echo "Copied IDL to $DEST"

# Verify SDK builds
cd "$ROOT/sdk"
npx tsc --noEmit 2>&1 || {
  echo "Error: SDK type check failed after IDL sync"
  exit 1
}

echo "IDL synced and SDK compiles."
