use ethers::{
    prelude::*,
    types::transaction::eip2718::TypedTransaction,
    abi::Token,
};
use std::{sync::Arc, str::FromStr, time::Duration};
use eyre::Result;

/// Simulates Strategy 1: Two-Way Classic Spread Arbitrage using the FlashExecutor contract.
/// This is a local simulation only — it assumes Anvil is running and both contracts are deployed.
#[tokio::main]
async fn main() -> Result<()> {
    // === Setup signer and provider to Anvil (localhost:8545) ===
    let provider = Provider::<Http>::try_from("http://localhost:8545")?
        .interval(Duration::from_millis(10));
    let wallet: LocalWallet = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
        .parse::<LocalWallet>()?
        .with_chain_id(31337u64);
    let client = Arc::new(SignerMiddleware::new(provider, wallet));

    // === Address of deployed FlashExecutor contract ===
    let executor_address = Address::from_str("0x8464135c8f25da09e49bc8782676a84730c318bc")?;

    // === Run flashloan simulation ===
    simulate_flashloan(client, executor_address).await
}

/// Internal logic for calling FLASH_LOAN_404 macro with mock inputs.
/// Order: token, amount, dex1, dex2, initiator.
pub async fn simulate_flashloan(
    client: Arc<SignerMiddleware<Provider<Http>, Wallet<k256::ecdsa::SigningKey>>>,
    executor_address: Address,
) -> Result<()> {
    let token_address = Address::from_str("0x4200000000000000000000000000000000000006")?;
    let amount = U256::from_dec_str("1000000000000000000")?;
    let dex1 = Address::from_str("0x1111111254EEB25477B68fb85Ed929f73A960582")?;
    let dex2 = Address::from_str("0xE592427A0AEce92De3Edee1F18E0157C05861564")?;
    let initiator = client.address();

    let calldata = ethers::abi::encode(&[
        Token::Address(token_address),
        Token::Uint(amount),
        Token::Address(dex1),
        Token::Address(dex2),
        Token::Address(initiator),
    ]);

    let tx = TypedTransaction::Legacy(TransactionRequest {
        to: Some(NameOrAddress::Address(executor_address)),
        data: Some(calldata.into()),
        ..Default::default()
    });

    let receipt = client.send_transaction(tx, None).await?
        .await?
        .ok_or_else(|| eyre::eyre!("Transaction failed or reverted"))?;

    println!("✅ FlashLoan executed. Tx Hash: {:?}", receipt.transaction_hash);
    Ok(())
}
