# ulp-1-5

## Note ulp.1.5
Ensure you have the most current `general_guide.md` and `ulp1.5.md`.

## Get Rust Going on WSL

### Clean Up
```bash
rm -f Cargo.lock
cargo clean
```

### Run the Bot
```bash
cargo run --bin ulp1_5
```

## Fire Up Anvil - Optimism
```bash
anvil --fork-url https://mainnet.optimism.io
```

## Compile huff contract
``` bash
huffc ./contracts/ArbitrageExecutor.huff -b > ./build/ArbitrageExecutor.bin
```

## error check huff contract (verbose output)
``` bash
huffc ./contracts/ArbitrageExecutor.huff -v
```

### Deploy Contract (note secret.env)
```bash
cast send --rpc-url http://127.0.0.1:8545 --private-key <YOUR_ANVIL_PK> --create <BYTECODE_HEX_STRING>
```

Replace `<YOUR_ANVIL_PK>` with your private key and `<BYTECODE_HEX_STRING>` with the contract bytecode.

## Integration with ULP 1.5

### 🚀 Overview
ULP 1.5 provides the foundation to enable arbitrage across 20+ Layer 2 DEXs using Balancer flash loans and ultra-low latency Huff executors.

### 🧪 Local Simulation
Use [Foundry's Anvil](https://book.getfoundry.sh/anvil/) for local forking and live simulations:
```bash
anvil --fork-url https://mainnet.optimism.io --chain-id 10
```
