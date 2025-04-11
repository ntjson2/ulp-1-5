use ethers::prelude::*;
use std::{sync::Arc, time::Duration};
use eyre::Result;
use hex::decode;
use std::fs;

#[tokio::main]
async fn main() -> Result<()> {
    // === Setup provider and wallet ===
    let provider = Provider::<Http>::try_from("http://localhost:8545")?
        .interval(Duration::from_millis(10u64));
    let wallet: LocalWallet = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
        .parse()?;
    let client = SignerMiddleware::new(provider, wallet.with_chain_id(31337u64));
    let client = Arc::new(client);

    // === Deploy FlashExecutor (compiled with huff-neo) ===
    let flash_bytecode_raw = fs::read("build/flash_executor.bin")?;
    let flash_factory = ContractFactory::new(
        ethers::abi::Abi::default(),
        Bytes::from(flash_bytecode_raw),
        client.clone(),
    );
    let flash_contract = flash_factory.deploy(())?.send().await?;
    println!("FlashExecutor deployed at: {:?}", flash_contract.address());

    // === Deploy UniV4Swapper (compiled with huff-neo) ===
    let swap_bytecode_raw = fs::read("build/uni_v4_swapper.bin")?;
    let swap_factory = ContractFactory::new(
        ethers::abi::Abi::default(),
        Bytes::from(swap_bytecode_raw),
        client.clone(),
    );
    let swap_contract = swap_factory.deploy(())?.send().await?;
    println!("UniV4Swapper deployed at: {:?}", swap_contract.address());

    // === Next Step Placeholder ===
    // Simulate a call from FlashExecutor to UniV4Swapper here
    // Log the gas used and estimated profit (if any) for verification

    Ok(())
}