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

## Get rust going on WSL

Clean Up
-- rm -f Cargo.lock
-- cargo clean

## Run the Bot
cargo run --bin ulp1_5

## Fireup anvil - optimism
anvil --fork-url https://mainnet.optimism.io/

cast send --rpc-url http://127.0.0.1:8545 --private-key <YOUR_ANVIL_PK> --create <BYTECODE_HEX_STRING>

## Compile from Huff
Install legacy Huff:
curl -L get.huff.sh | bash
source ~/.bashrc  # or ~/.zshrc
huffup             # installs the latest stable huffc

Compile
huffc ./contracts/ArbitrageExecutor.huff -b > ./build/deploy.bin

Huff-Neo (todo)
../tools/huff-neo.exe UniV4Swapper.huff -r > ../build/uni_v4_swapper.bin --alt-main UNI_V4_SWAP

## to build folder (old techniques)
$hex = "60538060093d393df360043560243560443573ba12222222228d8ba445958a75a0704d566bf2c95af160643560843563a9059cbb5f52906020529160405260605ff173ba12222222228d8ba445958a75a0704d566bf2c95f5f5f5af1"
$bytes = for ($i = 0; $i -lt $hex.Length; $i += 2) { [Convert]::ToByte($hex.Substring($i, 2), 16) }
[IO.File]::WriteAllBytes("build\flash_executor.bin", $bytes)

Bytecode saved successfully to build\flash_executor.bin using Huff-Neo output!

$hex = "60018060093d393df300"
$bytes = for ($i = 0; $i -lt $hex.Length; $i += 2) { [Convert]::ToByte($hex.Substring($i, 2), 16) }
[IO.File]::WriteAllBytes("build\simple_arb.bin", $bytes)


## Ready to test deploy?
cast send --rpc-url http://127.0.0.1:8545 --private-key <YOUR_ANVIL_PK> --create <BYTECODE_HEX_STRING>
ex:
cast send --rpc-url http://127.0.0.1:8545 --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 --create 335f55006102d480600e3d393df35f3560e01c8063f3f18ace14610029578063fa461e33146101e757806301e3b41a14610275575f5ffd5b602435602001355f5260443560200135602052600435602001356040526064358060240135806044013590508060640135905080608401359050508015610089577801461446703485210103287273052203988822378723970342610093565b6442951287396001015b6040516080528260806020015263022c0d9f61010052306101006004015261010060240181525f5161010060440152610100606401905260a061010060840152604061010060a4015260805161010060c4015260806020015161010060e40152925f5f6101046101005f905af11561026e15801561012a577801461446703485210103287273052203988822378723970342610134565b6442951287396001015b63022c0d9f610100523061010060040152610100602401815260605161010060440152610100606401905260a061010060840152915f5f6101046101005f905af11561026e50506020515f510163095ea7b36101005273ba12222222228d8ba445958a75a0704d566bf2c96101006004015261010060240190525f5f60446101005f6040515af11561026e7fa157427a8d45e187257fa91ff98f73367a3e04075e180055503bf726067157a95f5260205ff35b600435602435805f1161022c575f9003906060526044356024013563a9059cbb61010052336101006004015261010060240190525f5f60446101005f905af11561026e005b606052805f1161026c575f90036044356024016020013563a9059cbb61010052336101006004015261010060240190525f5f60446101005f905af11561026e5b005b1515575f5ffd5b335f5414610281575f5ffd5b6004356024356370a0823161010052306101006004015260205f60246101005f82fa1561026e5f5163a9059cbb6101005290916101006004015290610100602401525f5f60446101005f905af11561026e00

## Success?
Deployed Contract Address: 0x70E5370b8981Abc6e14C91F4AcE823954EFC8eA3
blockHash            0x091cf43d3d9fac2ed79a674d07ba85e2f392b0e4aa54c9c1c1bfd1f3fe5c1eca
blockNumber          134418130
contractAddress      0x70E5370b8981Abc6e14C91F4AcE823954EFC8eA3
cumulativeGasUsed    86432
effectiveGasPrice    1419452
from                 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
gasUsed              86432
root
status               1 (success)
transactionHash      0x7cf535cf6f9b4939f36b5ca44f998de325f11fcb1b4ced4bde0fdd1fbd64dc34
transactionIndex     0
type                 2
blobGasPrice         1
blobGasUsed
authorizationList
