#!/usr/bin/env bash
set -euo pipefail

ANVIL_PORT=8545
ANVIL_RPC="http://127.0.0.1:${ANVIL_PORT}"
FORK_URL="https://mainnet.optimism.io"
CHAIN_ID=10

# ----------------------------------------------------------------------------------
# 1. Launch anvil in the background
# ----------------------------------------------------------------------------------
echo "🔌 Spawning anvil …"
anvil --fork-url "${FORK_URL}" \
      --chain-id  "${CHAIN_ID}" \
      --port      "${ANVIL_PORT}" \
      --silent    \
      > /dev/null 2>&1 &
ANVIL_PID=$!

# Make sure we always clean up
cleanup() {
  echo -e "\n🛑 Shutting down anvil (pid $ANVIL_PID)…"
  kill "$ANVIL_PID"
}
trap cleanup INT TERM EXIT

# Give anvil a couple seconds to start
sleep 3

# Give anvil a moment to start listening
echo "⏳ Waiting for Anvil RPC at ${ANVIL_RPC}..."
until echo > /dev/tcp/127.0.0.1/${ANVIL_PORT}; do
  sleep 1
done
echo "✅ Anvil is ready."

# ----------------------------------------------------------------------------------
# 2. Deploy ArbitrageExecutor with cast
# ----------------------------------------------------------------------------------
echo "🚀 Deploying ArbitrageExecutor …"
PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
BYTECODE="0x$(cat ./build/ArbitrageExecutor.bin)"

DEPLOY_TX=$(cast send \
  --rpc-url     "${ANVIL_RPC}" \
  --legacy \
  --private-key "${PRIVATE_KEY}" \
  --create "${BYTECODE}" \
  --json         )     # ← returns full JSON

DEPLOY_ADDR=$(echo "${DEPLOY_TX}" | jq -r '.contractAddress')

echo "✅ ArbitrageExecutor deployed to ${DEPLOY_ADDR}"

# Export for the Rust tests (they read ARB_EXECUTOR_ADDRESS)
export ARBITRAGE_EXECUTOR_ADDRESS="${DEPLOY_ADDR}"

# Optional: trim the optimiser search space for faster CI runs
export OPTIMAL_LOAN_SEARCH_ITERATIONS=1
export MAX_LOAN_AMOUNT_WETH=1

# ----------------------------------------------------------------------------------
# 3. Run ignored integration tests single‑threaded
# ----------------------------------------------------------------------------------
echo "🧪 Running integration tests …"
RUST_TEST_THREADS=1 \
cargo test --features local_simulation -- --ignored --nocapture
