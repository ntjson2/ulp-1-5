// src/config.rs

// --- Imports ---
// Import necessary types and functions from libraries
use ethers::types::Address; // Used for Ethereum addresses
use eyre::Result; // Used for convenient error handling (allows using `?` for propagation)
use std::{env, str::FromStr}; // `env` for reading environment variables, `FromStr` for parsing strings
use dotenv::dotenv; // For loading variables from a .env file

// --- Configuration Struct ---
// Defines a structure to hold all application configuration parameters.
// Deriving Debug allows printing the struct easily for debugging.
// Deriving Clone allows creating copies of the configuration if needed.
#[derive(Debug, Clone)]
pub struct Config {
    // RPC URL for connecting to the blockchain node (e.g., Anvil, Alchemy)
    pub local_rpc_url: String,
    // Private key for the wallet used to send transactions (should start with 0x)
    pub local_private_key: String,
    // Deployed address of our custom arbitrage executor Huff contract
    pub arb_executor_address: Address,
    // Address of the target Uniswap V3 WETH/USDC pool
    pub uni_v3_pool_addr: Address,
    // Address of the target Velodrome V2 WETH/USDC pool
    pub velo_v2_pool_addr: Address,
    // Address of the Wrapped Ether (WETH) token contract on the target chain
    pub weth_address: Address,
    // Address of the USDC token contract on the target chain
    pub usdc_address: Address,
    // Number of decimals for the WETH token (usually 18)
    pub weth_decimals: u8,
    // Number of decimals for the USDC token (usually 6)
    pub usdc_decimals: u8,
    // Address of the Velodrome V2 Router contract
    pub velo_router_addr: Address,
    // Address of the Balancer V2 Vault contract (used for flash loans)
    pub balancer_vault_address: Address,
    // Address of the Uniswap V3 QuoterV2 contract (used for swap simulations)
    pub quoter_v2_address: Address,
}

// --- Load Configuration Function ---
// This function reads environment variables, parses them, and populates the Config struct.
// It's marked `pub` so it can be called from other modules (like main.rs).
pub fn load_config() -> Result<Config> {
    println!("Loading configuration from .env file...");
    // Attempt to load variables from a `.env` file in the project root.
    // `.ok()` ignores errors if the file doesn't exist (variables must be set in the environment).
    dotenv().ok();

    // Create the Config struct by reading and parsing each environment variable.
    // `.expect()` will cause the program to panic if a variable is not set.
    // `.parse::<Type>()?` attempts to convert the string variable to the specified type (e.g., Address, u8).
    // The `?` operator propagates any parsing errors up the call stack.
    let config = Config {
        // Read RPC URL string
        local_rpc_url: env::var("LOCAL_RPC_URL")
            .expect("❌ Environment variable LOCAL_RPC_URL must be set"),
        // Read Private Key string
        local_private_key: env::var("LOCAL_PRIVATE_KEY")
            .expect("❌ Environment variable LOCAL_PRIVATE_KEY must be set"),
        // Read and parse Arbitrage Executor address
        arb_executor_address: env::var("ARBITRAGE_EXECUTOR_ADDRESS")
            .expect("❌ Environment variable ARBITRAGE_EXECUTOR_ADDRESS must be set")
            .parse::<Address>()?, // Parse string to Address
        // Read and parse Uniswap V3 Pool address
        uni_v3_pool_addr: env::var("UNI_V3_POOL_ADDR")
            .expect("❌ Environment variable UNI_V3_POOL_ADDR must be set")
            .parse::<Address>()?,
        // Read and parse Velodrome V2 Pool address
        velo_v2_pool_addr: env::var("VELO_V2_POOL_ADDR")
            .expect("❌ Environment variable VELO_V2_POOL_ADDR must be set")
            .parse::<Address>()?,
        // Read and parse WETH token address
        weth_address: env::var("WETH_ADDRESS")
            .expect("❌ Environment variable WETH_ADDRESS must be set")
            .parse::<Address>()?,
        // Read and parse USDC token address
        usdc_address: env::var("USDC_ADDRESS")
            .expect("❌ Environment variable USDC_ADDRESS must be set")
            .parse::<Address>()?,
        // Read and parse WETH decimals
        weth_decimals: env::var("WETH_DECIMALS")? // Use `?` for potential error if var exists but isn't a number
            .parse::<u8>() // Parse string to u8 (unsigned 8-bit integer)
            .expect("❌ WETH_DECIMALS must be a valid number (0-255)"),
        // Read and parse USDC decimals
        usdc_decimals: env::var("USDC_DECIMALS")?
            .parse::<u8>()
            .expect("❌ USDC_DECIMALS must be a valid number (0-255)"),
        // Read and parse Velodrome Router address
        velo_router_addr: env::var("VELO_V2_ROUTER_ADDR")
            .expect("❌ Environment variable VELO_V2_ROUTER_ADDR must be set")
            .parse::<Address>()?,
        // Read and parse Balancer Vault address
        balancer_vault_address: env::var("BALANCER_VAULT_ADDRESS")
            .expect("❌ Environment variable BALANCER_VAULT_ADDRESS must be set")
            .parse::<Address>()?,
        // Read and parse Quoter V2 address
        quoter_v2_address: env::var("QUOTER_V2_ADDRESS")
            .expect("❌ Environment variable QUOTER_V2_ADDRESS must be set")
            .parse::<Address>()?,
    };

    println!("✅ Configuration loaded successfully.");
    // Return the populated Config struct, wrapped in Ok to signify success.
    Ok(config)
}