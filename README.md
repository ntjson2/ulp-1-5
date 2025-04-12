# ulp-1-5

##  
ULP 1.5 — Unified Liquidity Profiteer (GLP1.0 Core)
ULP 1.5 is a high-efficiency MEV arbitrage system built on Huff2 + Rust, optimized for L2 execution (Optimism, Base, Arbitrum). It implements the GLP1.0 strategy suite:

- 2-Way Classic Spread Arbitrage

- 3-Way Triangular Arbitrage

- 10-Pair Scanner Arbitrage

- High-Frequency Micro Trades

- Stablecoin Delta Arbitrage

The system uses:

Huff-Neo flashloan contracts for atomic execution with ultra-low gas overhead

Rust-based bot engine (ethers-rs + tokio) for real-time subgraph polling and trade execution

Balancer flashloans + multi-DEX routing (Uniswap V4, Velodrome, Ramses)

📈 Target ROI: $500–$40,800/day from realistic L2 arbitrage opportunities
🖥️ Deployment-ready on GCP VPS with full automation support

## Fireup anvil - optimism
anvil --fork-url https://mainnet.optimism.io/

## Compile from Huff
.\tools\huff-neo.exe contracts\FlashExecutor.huff --bytecode --alt-main FLASH_LOAN_404


../tools/huff-neo.exe SimpleArb.huff -r > ../build/SimpleArb.bin
../tools/huff-neo.exe FlashExecutor.huff -r > ../build/flash_executor.bin --alt-main FLASH_LOAN_404
../tools/huff-neo.exe UniV4Swapper.huff -r > ../build/uni_v4_swapper.bin --alt-main UNI_V4_SWAP

../tools/huff-neo.exe ArbitrageExecutor.huff -c > deploy.bin



## to build folder
$hex = "60538060093d393df360043560243560443573ba12222222228d8ba445958a75a0704d566bf2c95af160643560843563a9059cbb5f52906020529160405260605ff173ba12222222228d8ba445958a75a0704d566bf2c95f5f5f5af1"
$bytes = for ($i = 0; $i -lt $hex.Length; $i += 2) { [Convert]::ToByte($hex.Substring($i, 2), 16) }
[IO.File]::WriteAllBytes("build\flash_executor.bin", $bytes)

Bytecode saved successfully to build\flash_executor.bin using Huff-Neo output!

$hex = "60018060093d393df300"
$bytes = for ($i = 0; $i -lt $hex.Length; $i += 2) { [Convert]::ToByte($hex.Substring($i, 2), 16) }
[IO.File]::WriteAllBytes("build\simple_arb.bin", $bytes)
