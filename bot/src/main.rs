use ethers::prelude::*;
use eyre::Result;
use std::env;
use std::sync::Arc;
use dotenv::dotenv;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok(); // Load .env file from the current directory (bot/)

    // Load environment variables
    let rpc_url = env::var("LOCAL_RPC_URL").expect("LOCAL_RPC_URL must be set in .env");
    let private_key = env::var("LOCAL_PRIVATE_KEY").expect("LOCAL_PRIVATE_KEY must be set in .env");
    let contract_addr_str = env::var("ARBITRAGE_EXECUTOR_ADDRESS")
        .expect("ARBITRAGE_EXECUTOR_ADDRESS must be set in .env");

    println!("Connecting to Anvil node at: {}", rpc_url);

    // Setup provider
    let provider = Provider::<Http>::try_from(rpc_url)?;
    let provider = Arc::new(provider);

    // Get chain ID from the connected node (Anvil fork retains the original chain ID)
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("Connected to chain ID: {}", chain_id);

    // Setup wallet/signer
    let wallet = private_key
        .parse::<LocalWallet>()?
        .with_chain_id(chain_id); // Crucial for correct transaction signing
    let signer_address = wallet.address();
    println!("Using signer address: {:?}", signer_address);

    // Setup client middleware (handles signing)
    let client = SignerMiddleware::new(provider.clone(), wallet.clone());
    let client = Arc::new(client);

    // Parse contract address
    let contract_address = contract_addr_str.parse::<Address>()?;
    println!("ArbitrageExecutor contract address: {:?}", contract_address);

    // Get current block number from the Anvil fork
    let block_number = provider.get_block_number().await?;
    println!("Current Anvil block number: {}", block_number);

    // Get signer balance (in Ether)
    let balance = provider.get_balance(signer_address, None).await?;
    // Convert Wei to Ether for display
    let balance_ether = ethers::utils::format_units(balance, "ether")?;
    println!("Signer balance: {} ETH", balance_ether);


    println!("\n--- Basic Setup Complete ---");

    // --- Next Steps: ---
    // 1. Implement data polling (Uniswap pool prices/liquidity)
    // 2. Implement arbitrage opportunity detection logic
    // 3. Implement userData encoding
    // 4. Implement flash loan transaction execution


    Ok(())
}