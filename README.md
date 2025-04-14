# ulp-1-5

##  Note ulp.1.5
Check for most current general_guide.md and ulp1.5 md

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
huffup             # installs the latest stableanvi huffc

Compile

## Make sure you are in the ulp-1.5 directory

* huffc ./contracts/ArbitrageExecutor.huff -b > ./build/deploy_v2_1_0.bin

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
cast send --rpc-url http://127.0.0.1:8545 --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 --create <contract data>

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
