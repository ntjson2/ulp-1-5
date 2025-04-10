use ethers::prelude::*;
use ethers::abi::Abi;
use std::{convert::TryFrom, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Read the compiled Huff bytecode from the correct file
    let bytecode_raw = std::fs::read("build/uni_v4_swapper.bin")?;
    let bytecode_bytes = Bytes::from(bytecode_raw);

    // Set up the Anvil local provider
    let provider = Provider::<Http>::try_from("http://localhost:8545")?
        .interval(Duration::from_millis(10));

    // Use the default dev key from Anvil (account #0)
    let wallet: LocalWallet = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        .parse::<LocalWallet>()?
        .with_chain_id(31337u64);

    let client = SignerMiddleware::new(provider, wallet);
    let client = Arc::new(client);

    // Use an empty ABI (Huff contract doesn't expose methods yet)
    let empty_abi: Abi = Abi::default();

    // Deploy contract using ethers-rs
    let factory = ContractFactory::new(empty_abi, bytecode_bytes, client.clone());
    let contract = factory.deploy(())?.send().await?;

    println!("✅ UniV4Swapper deployed at: {}", contract.address());

    Ok(())
}
