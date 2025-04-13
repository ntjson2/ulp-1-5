use ethers::{
    prelude::*,
    types::{U256}, // Removed I256 as it's not used directly here
    utils::format_units, // Keep for displaying balance if needed later
};
use eyre::Result;
use std::{env, sync::Arc, time::Duration};
use dotenv::dotenv;
use std::str::FromStr; // For parsing U256 to f64 helper

// Define contract bindings using ABIs
abigen!(
    UniswapV3Pool,
    "./abis/UniswapV3Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    VelodromeV2Pool,
    "./abis/VelodromeV2Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

// --- Constants ---
// Define the minimum spread percentage required to attempt simulation/execution
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; // Example: 0.1%

// --- Helper Functions ---

// Calculate Uniswap V3 price from sqrtPriceX96
// Returns price of token0 in terms of token1
fn v3_price_from_sqrt(sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if sqrt_price_x96.is_zero() { return Ok(0.0); } // Handle pool not initialized
    let price_ratio_x192 = sqrt_price_x96.pow(U256::from(2));
    let q96 = U256::from(2).pow(U256::from(96));
    let q192 = q96.pow(U256::from(2));
    let price_ratio_f = u256_to_f64(price_ratio_x192)? / u256_to_f64(q192)?;
    let decimal_diff = (decimals0 as i16) - (decimals1 as i16);
    let adjustment = 10f64.powi(decimal_diff as i32);
    Ok(price_ratio_f * adjustment)
}

// Calculate Uniswap V2/Velodrome price from reserves
// Returns price of token0 in terms of token1
fn v2_price_from_reserves(reserve0: U256, reserve1: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if reserve0.is_zero() { return Ok(0.0); } // Check for empty reserves
    let reserve0_f = u256_to_f64(reserve0)?;
    let reserve1_f = u256_to_f64(reserve1)?;
    let price_ratio_f = reserve1_f / reserve0_f;
    let decimal_diff = (decimals0 as i16) - (decimals1 as i16);
    let adjustment = 10f64.powi(decimal_diff as i32);
    Ok(price_ratio_f * adjustment)
}

// Helper to convert U256 to f64
fn u256_to_f64(value: U256) -> Result<f64> {
    f64::from_str(&value.to_string()).map_err(|e| eyre::eyre!("Failed to parse U256 as f64: {}", e))
}

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok(); // Load .env file from the project root

    // --- Load Configuration ---
    let rpc_url = env::var("LOCAL_RPC_URL").expect("LOCAL_RPC_URL must be set");
    let _private_key = env::var("LOCAL_PRIVATE_KEY").expect("LOCAL_PRIVATE_KEY must be set");
    let arb_executor_address_str = env::var("ARBITRAGE_EXECUTOR_ADDRESS")
        .expect("ARBITRAGE_EXECUTOR_ADDRESS must be set");
    let uni_v3_pool_addr_str = env::var("UNI_V3_POOL_ADDR").expect("UNI_V3_POOL_ADDR must be set");
    let velo_v2_pool_addr_str = env::var("VELO_V2_POOL_ADDR").expect("VELO_V2_POOL_ADDR must be set");
    let weth_addr_str = env::var("WETH_ADDRESS").expect("WETH_ADDRESS must be set");
    let usdc_addr_str = env::var("USDC_ADDRESS").expect("USDC_ADDRESS must be set");
    let weth_decimals: u8 = env::var("WETH_DECIMALS").expect("WETH_DECIMALS must be set").parse()?;
    let usdc_decimals: u8 = env::var("USDC_DECIMALS").expect("USDC_DECIMALS must be set").parse()?;
    let _velo_router_addr_str = env::var("VELO_V2_ROUTER_ADDR").expect("VELO_V2_ROUTER_ADDR must be set"); // Load but mark unused for now

    println!("--- Configuration ---");
    println!("Arb Contract: {}", arb_executor_address_str);
    println!("Watching UniV3 Pool (WETH/USDC 0.05%?): {}", uni_v3_pool_addr_str);
    println!("Watching VeloV2 Pool (WETH/USDC Vol?): {}", velo_v2_pool_addr_str);
    // println!("WETH Addr: {}", weth_addr_str); // Less verbose logging
    // println!("USDC Addr: {}", usdc_addr_str);
    println!("Arb Threshold: {:.4}%", ARBITRAGE_THRESHOLD_PERCENTAGE);


    // --- Setup Provider ---
    let provider = Provider::<Http>::try_from(rpc_url)?;
    let provider = Arc::new(provider);
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("RPC OK. Chain ID: {}", chain_id);


    // --- Parse Addresses ---
    let uni_v3_pool_address = uni_v3_pool_addr_str.parse::<Address>()?;
    let velo_v2_pool_address = velo_v2_pool_addr_str.parse::<Address>()?;
    let weth_address = weth_addr_str.parse::<Address>()?;
    let usdc_address = usdc_addr_str.parse::<Address>()?;
    let _arb_executor_address = arb_executor_address_str.parse::<Address>()?;


    // --- Create Contract Instances ---
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, provider.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, provider.clone());


    // --- Determine Token Order & Decimals ---
    let velo_token0 = velo_v2_pool.token_0().call().await?;
    let velo_token1 = velo_v2_pool.token_1().call().await?;
    let (velo_decimals0, velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address {
        println!("Velo Pool Order: Token0=WETH, Token1=USDC");
        (weth_decimals, usdc_decimals, true)
    } else if velo_token0 == usdc_address && velo_token1 == weth_address {
         println!("Velo Pool Order: Token0=USDC, Token1=WETH");
        (usdc_decimals, weth_decimals, false)
    } else {
        eyre::bail!("Velo pool tokens do not match WETH/USDC addresses in .env");
    };
    // Assuming UniV3 uses WETH as Token0 for price calculation consistency
    let uni_decimals0 = weth_decimals;
    let uni_decimals1 = usdc_decimals;
    let uni_t0_is_weth = true; // Based on standard pool convention


    println!("\n--- Starting Polling Loop (Ctrl+C to stop) ---");


    // --- Polling Loop ---
    let mut poll_interval = tokio::time::interval(Duration::from_secs(10));

    loop {
        poll_interval.tick().await;
        let current_block = provider.get_block_number().await?;
        println!("\n--- Block: {} ---", current_block);

                // --- Fetch UniV3 State & Calculate Price ---
         // --- Fetch UniV3 State & Calculate Price ---
         let uni_price_result = async {
            let slot0_data = uni_v3_pool.slot_0().call().await?;
            // FIX: Access the first element (index 0) of the tuple for sqrtPriceX96
            let sqrt_price_x96 = slot0_data.0;
            println!("UniV3 SqrtPriceX96: {}", sqrt_price_x96);
            let price = v3_price_from_sqrt(sqrt_price_x96, uni_decimals0, uni_decimals1)?;
            println!("  -> UniV3 WETH Price (USDC): {:.6}", price);
            Ok::<_, eyre::Report>(price) // Explicit type annotation for clarity
        }.await;
               // --- Fetch Velodrome State & Calculate Price ---
        let velo_price_result = async {
             let reserves = velo_v2_pool.get_reserves().call().await?;
             let price = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), velo_decimals0, velo_decimals1)?;
             // Adjust price if WETH is not token0 in Velo pool
             let adjusted_price = if velo_t0_is_weth { price } else { 1.0 / price };
             println!("VeloV2 Price (WETH/USDC): {:.6}", adjusted_price);
             Ok::<_, eyre::Report>(adjusted_price)
        }.await;


        // --- Arbitrage Detection ---
        match (uni_price_result, velo_price_result) {
            (Ok(p_uni), Ok(p_velo)) => {
                // Calculate absolute spread percentage
                let price_diff = (p_uni - p_velo).abs();
                // Use the lower price for the denominator to get a clearer arb % margin
                let base_price = p_uni.min(p_velo);
                let spread_percentage = if base_price > 1e-18 { // Avoid division by zero if prices are near zero
                    (price_diff / base_price) * 100.0
                } else {
                    0.0
                };

                println!("-> Spread (Abs): {:.4}%", spread_percentage);

                // Check if spread exceeds threshold
                if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                    println!(">>> Arbitrage Opportunity DETECTED! Spread: {:.4}%", spread_percentage);
                    // Determine direction: Which price is higher?
                    if p_uni > p_velo {
                        println!("    Direction: Buy Velo (Low: {:.6}), Sell UniV3 (High: {:.6})", p_velo, p_uni);
                        // TODO: Step 7a: Simulate Velo -> UniV3 trade (check liquidity, estimate gas, fees, slippage)
                        // TODO: Step 7b: If profitable, encode userData (is_A_Velo=1, is_B_Velo=0, zeroForOne_A=?, ...)
                        // TODO: Step 7c: Prepare and send flash loan transaction to executor contract
                    } else {
                         println!("    Direction: Buy UniV3 (Low: {:.6}), Sell Velo (High: {:.6})", p_uni, p_velo);
                         // TODO: Step 7a: Simulate UniV3 -> Velo trade (check liquidity, estimate gas, fees, slippage)
                         // TODO: Step 7b: If profitable, encode userData (is_A_Velo=0, is_B_Velo=1, zeroForOne_A=?, ...)
                         // TODO: Step 7c: Prepare and send flash loan transaction to executor contract
                    }

                } else {
                    //println!("-> Spread below threshold ({:.4}%)", ARBITRAGE_THRESHOLD_PERCENTAGE); // Reduce log noise
                }
            },
            (Err(e), _) => eprintln!("Error getting UniV3 price: {}", e),
            (_, Err(e)) => eprintln!("Error getting VeloV2 price: {}", e),
        }
    }
}