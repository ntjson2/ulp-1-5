// src/deploy.rs

use ethers::{
    abi::Abi,
    // Removed unused Middleware
    prelude::{ContractFactory, SignerMiddleware, Provider, Http, LocalWallet},
    types::{Address, Bytes},
    utils::hex,
};
use eyre::{Result, WrapErr};
// Removed unused future::Future
use std::{path::Path, fs, sync::Arc};

/// Deploys a contract from raw bytecode.
pub async fn deploy_contract_from_bytecode(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    bytecode_path: impl AsRef<Path>,
) -> Result<Address> {
    let path_ref = bytecode_path.as_ref();
    println!("      Deploying contract from bytecode file: {:?}", path_ref);

    // 1. Read bytecode
    let bytecode_hex = fs::read_to_string(path_ref)
        .wrap_err_with(|| format!("Failed to read bytecode file: {:?}", path_ref))?;
    let cleaned_bytecode_hex = bytecode_hex.trim().trim_start_matches("0x");

    // 2. Decode hex bytecode
    let bytecode = hex::decode(cleaned_bytecode_hex)
        .wrap_err("Failed to decode hex bytecode")?;
    let deploy_bytes = Bytes::from(bytecode);

    // 3. Construct ContractFactory with correct argument order
    let factory = ContractFactory::new(
        Abi::default(), // Provide an empty ABI
        deploy_bytes,   // Provide the deployment bytecode
        client.clone(), // Clone the Arc for the factory
    );

    // Prepare the deployment call with empty constructor arguments `()`
    let deployer = factory.deploy(())
        .map_err(|e| eyre::eyre!("Failed to construct deployment call: {}", e))?;

    println!("      Sending deployment transaction...");
    // 4. Send the deployment transaction
    let contract_instance_future = deployer.send().await
        .wrap_err("Failed to send deployment transaction")?;

    // 5. Get Address Directly (Doesn't wait for confirmation)
    let contract_address = contract_instance_future.address();
    println!("      ✅ Contract Deployed (Instance Received) at: {:?}", contract_address);
    // TODO: Add optional logic here to wait for the deployment transaction receipt
    // using contract_instance_future.deployer().tx_hash() and provider.get_transaction_receipt()
    // if full confirmation is required before proceeding.

    Ok(contract_address)
}