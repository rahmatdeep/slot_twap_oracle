#!/usr/bin/env bash
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PASSED=0
FAILED=0
FAILURES=""

run_step() {
  local name="$1"
  shift
  echo ""
  echo "════════════════════════════════════════════════════"
  echo "  $name"
  echo "════════════════════════════════════════════════════"
  if "$@"; then
    PASSED=$((PASSED + 1))
    echo "  ✓ $name"
  else
    FAILED=$((FAILED + 1))
    FAILURES="$FAILURES\n  ✗ $name"
    echo "  ✗ $name FAILED"
  fi
}

cleanup() {
  pkill -f solana-test-validator 2>/dev/null || true
}
trap cleanup EXIT

# ── 1. Build program ──
run_step "Anchor build" anchor build

# ── 2. Rust tests (LiteSVM — 61 tests) ──
run_step "Rust integration tests" cargo test --manifest-path tests/Cargo.toml

# ── 3. SDK type check + build ──
run_step "SDK type check" bash -c "cd sdk && npx tsc --noEmit"
run_step "SDK build" bash -c "cd sdk && npm run build"

# ── 4. Re-link SDK for scripts ──
echo ""
echo "Linking SDK..."
(cd sdk && npm link) >/dev/null 2>&1
npm link @slot-twap-oracle/sdk >/dev/null 2>&1

# ── 5. SDK mocha tests (18 tests — starts its own validator) ──
run_step "SDK mocha tests" npx tsx node_modules/.bin/mocha --timeout 120000 sdk-tests/oracle.test.ts

# ── 6. E2E test (starts its own validator) ──
pkill -f solana-test-validator 2>/dev/null || true
sleep 1
run_step "E2E integration test" npx tsx scripts/e2e-test.ts 2>&1

# ── 7. Bot type check ──
run_step "Updater bot type check" bash -c "cd bots/updater && npx tsc --noEmit"

# ── Summary ──
echo ""
echo "════════════════════════════════════════════════════"
echo "  RESULTS: $PASSED passed, $FAILED failed"
echo "════════════════════════════════════════════════════"
if [ $FAILED -gt 0 ]; then
  echo -e "\nFailures:$FAILURES"
  exit 1
else
  echo ""
  echo "  All checks passed."
  exit 0
fi
