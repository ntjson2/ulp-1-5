use ethers::{
    prelude::*,
    types::{U256},
    // Only need format_units from utils now
    utils::{format_units},
};
use eyre::Result;
use std::{env, sync::Arc, str::FromStr}; // Removed Duration
use dotenv::dotenv;
// Removed Decimal imports

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
abigen!(
    VelodromeRouter,
    "./abis/VelodromeRouter.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    BalancerVault,
    "./abis/BalancerVault.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    IERC20,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);


// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1;
const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const SIMULATION_AMOUNT_WETH: f64 = 1.0;


// --- Helper Functions ---

// Helper extension trait for lossy U256 -> f64 conversion
trait ToF64Lossy { fn to_f64_lossy(&self) -> f64; }
impl ToF64Lossy for U256 {
    fn to_f64_lossy(&self) -> f64 {
        // Use string conversion for better range
        match f64::from_str(&self.to_string()) { Ok(f) => f, Err(_) => f64::MAX }
    }
}


// Calculate Uniswap V3 price from sqrtPriceX96 using f64
fn v3_price_from_sqrt(sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if sqrt_price_x96.is_zero() { return Ok(0.0); }
    let sqrt_price_x96_f64 = sqrt_price_x96.to_f64_lossy();
    let q96_f64: f64 = 2.0_f64.powi(96);
    if q96_f64 == 0.0 { return Err(eyre::eyre!("Q96 calculation resulted in zero")); }
    let price_ratio_f64 = (sqrt_price_x96_f64 / q96_f64).powi(2);
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    Ok(price_ratio_f64 * adjustment)
}

// Calculate Uniswap V2/Velodrome price from reserves using f64
fn v2_price_from_reserves(reserve0: U256, reserve1: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if reserve0.is_zero() { return Ok(0.0); }
    let reserve0_f64 = reserve0.to_f64_lossy();
    let reserve1_f64 = reserve1.to_f64_lossy();
    if reserve0_f64.abs() < f64::EPSILON { return Ok(0.0); }
    let price_ratio_f64 = reserve1_f64 / reserve0_f64;
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    Ok(price_ratio_f64 * adjustment)
}

// FIX: New helper function to parse float to U256 based on decimals
fn f64_to_wei(amount_f64: f64, decimals: u32) -> Result<U256> {
    if !amount_f64.is_finite() {
        return Err(eyre::eyre!("Cannot convert non-finite f64 to U256"));
    }
    // Use string formatting to handle decimals correctly
    let formatted_str = format!("{:.prec$}", amount_f64, prec = decimals as usize);
    // Remove the decimal point
    let parts: Vec<&str> = formatted_str.split('.').collect();
    let mut integer_part = parts[0].to_string();
    let fractional_part = if parts.len() > 1 { parts[1] } else { "" };

    // Combine integer and fractional parts, padding fractionals if necessary
    let mut full_str = integer_part;
    // Ensure fractional part length does not exceed decimals
    let fractional_part_trimmed = &fractional_part[..fractional_part.len().min(decimals as usize)];
    full_str.push_str(fractional_part_trimmed);

    // Pad with zeros if fractional part was shorter than decimals
    let current_len = fractional_part_trimmed.len();
    if current_len < decimals as usize {
        for _ in 0..(decimals as usize - current_len) {
            full_str.push('0');
        }
    }
    // Trim leading zeros unless it's just "0"
    let trimmed_str = full_str.trim_start_matches('0');
    let final_str = if trimmed_str.is_empty() { "0" } else { trimmed_str };

    U256::from_dec_str(final_str)
        .map_err(|e| eyre::eyre!("Failed to parse decimal string '{}' from f64 '{}' to U256: {}", final_str, amount_f64, e))
}


// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    // --- Load Configuration ---
    let rpc_url = env::var("LOCAL_RPC_URL").expect("LOCAL_RPC_URL must be set");
    let private_key = env::var("LOCAL_PRIVATE_KEY").expect("LOCAL_PRIVATE_KEY must be set");
    let arb_executor_address_str = env::var("ARBITRAGE_EXECUTOR_ADDRESS").expect("ARBITRAGE_EXECUTOR_ADDRESS must be set");
    let uni_v3_pool_addr_str = env::var("UNI_V3_POOL_ADDR").expect("UNI_V3_POOL_ADDR must be set");
    let velo_v2_pool_addr_str = env::var("VELO_V2_POOL_ADDR").expect("VELO_V2_POOL_ADDR must be set");
    let weth_addr_str = env::var("WETH_ADDRESS").expect("WETH_ADDRESS must be set");
    let usdc_addr_str = env::var("USDC_ADDRESS").expect("USDC_ADDRESS must be set");
    let weth_decimals: u8 = env::var("WETH_DECIMALS")?.parse()?;
    let usdc_decimals: u8 = env::var("USDC_DECIMALS")?.parse()?;
    let velo_router_addr_str = env::var("VELO_V2_ROUTER_ADDR").expect("VELO_V2_ROUTER_ADDR must be set");
    let balancer_vault_addr_str = env::var("BALANCER_VAULT_ADDRESS").expect("BALANCER_VAULT_ADDRESS must be set");

    // --- Setup Provider & Client ---
    let provider = Provider::<Http>::try_from(rpc_url)?;
    let provider = Arc::new(provider);
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("RPC OK. Chain ID: {}", chain_id);
    let wallet = private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let client = SignerMiddleware::new(provider.clone(), wallet.clone());
    let client = Arc::new(client);

    // --- Parse Addresses ---
    let uni_v3_pool_address = uni_v3_pool_addr_str.parse::<Address>()?;
    let velo_v2_pool_address = velo_v2_pool_addr_str.parse::<Address>()?;
    let weth_address = weth_addr_str.parse::<Address>()?;
    let usdc_address = usdc_addr_str.parse::<Address>()?;
    let _arb_executor_address = arb_executor_address_str.parse::<Address>()?;
    let _velo_router_address = velo_router_addr_str.parse::<Address>()?;
    let _balancer_vault_address = balancer_vault_addr_str.parse::<Address>()?;

    // --- Create Contract Instances ---
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, provider.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, provider.clone());
    let _velo_router = VelodromeRouter::new(_velo_router_address, provider.clone());
    let _balancer_vault = BalancerVault::new(_balancer_vault_address, client.clone());


    // --- Determine Token Order & Decimals ---
    let velo_token0 = velo_v2_pool.token_0().call().await?;
    let velo_token1 = velo_v2_pool.token_1().call().await?;
    let (velo_decimals0, velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address {
        (weth_decimals, usdc_decimals, true)
    } else if velo_token0 == usdc_address && velo_token1 == weth_address {
        (usdc_decimals, weth_decimals, false)
    } else {
        eyre::bail!("Velo pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", velo_token0, velo_token1);
    };
    let uni_decimals0 = weth_decimals;
    let uni_decimals1 = usdc_decimals;


    println!("\n--- Performing Single Test Run ---");

    let simulation_amount_weth_wei = f64_to_wei(SIMULATION_AMOUNT_WETH, weth_decimals as u32)?;

    // --- Fetch Prices ---
    let uni_price_result: Result<f64> = async {
        uni_v3_pool.slot_0().call().await
            .map_err(|e| eyre::eyre!("RPC Error fetching UniV3 slot0: {}", e))
            .and_then(|slot0_data| {
                v3_price_from_sqrt(slot0_data.0, uni_decimals0, uni_decimals1)
            })
    }.await;

    let velo_price_result: Result<f64> = async {
         velo_v2_pool.get_reserves().call().await
            .map_err(|e| eyre::eyre!("RPC Error fetching Velo reserves: {}", e))
            .and_then(|reserves| {
                let price = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), velo_decimals0, velo_decimals1)?;
                Ok(if velo_t0_is_weth { price } else { if price.abs() < f64::EPSILON { 0.0 } else { 1.0 / price } })
         })
    }.await;


    // --- Arbitrage Detection ---
        match (uni_price_result, velo_price_result) {
            (Ok(p_uni), Ok(p_velo)) => {
                 println!("UniV3 Price (WETH/USDC): {:.6}", p_uni);
                 println!("VeloV2 Price (WETH/USDC): {:.6}", p_velo);
                 let price_diff = (p_uni - p_velo).abs();
                 let base_price = p_uni.min(p_velo);
                 let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };
                 println!("-> Spread (Abs): {:.4}%", spread_percentage);

                 if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                    println!(">>> Arbitrage Opportunity DETECTED! Spread: {:.4}%", spread_percentage);

                    let mut is_profitable = false;
                    let mut net_profit_weth_wei = U256::zero();

                    let amount_in_wei = simulation_amount_weth_wei;
                    let token_in = weth_address;
                    let token_out = usdc_address;

                    let (buy_dex, sell_dex) = if p_uni > p_velo {
                        ("VeloV2", "UniV3")
                    } else {
                         ("UniV3", "VeloV2")
                    };
                    println!("    Direction: Buy {} (Low), Sell {} (High)", buy_dex, sell_dex);

                    // Simulate Swap 1 (Buy Low) - Placeholder using f64
                    let amount_out_intermediate_wei = {
                        let adjustment = 10f64.powi(weth_decimals as i32);
                        let amount_in_f64 = amount_in_wei.to_f64_lossy() / adjustment;
                        let price_f64 = if buy_dex == "VeloV2" { p_velo } else { p_uni };
                        let amount_out_f64 = amount_in_f64 * price_f64;
                        f64_to_wei(amount_out_f64, usdc_decimals as u32)? // Use new helper
                    };
                    let intermediate_decimals = if token_out == usdc_address { usdc_decimals } else { weth_decimals };
                    println!("    Simulated Swap 1 Output ({}): {} ({})",
                        if token_out == usdc_address {"USDC"} else {"WETH"},
                        amount_out_intermediate_wei,
                        format_units(amount_out_intermediate_wei, intermediate_decimals as u32)?
                    );

                    // Simulate Swap 2 (Sell High) - Placeholder using f64
                    let amount_out_final_wei = {
                         let adjustment = 10f64.powi(usdc_decimals as i32);
                         let amount_in_f64 = amount_out_intermediate_wei.to_f64_lossy() / adjustment;
                         let price_f64 = if sell_dex == "VeloV2" { p_velo } else { p_uni };
                         if price_f64.abs() < 1e-18 { U256::zero() } else {
                             let amount_out_f64 = amount_in_f64 / price_f64;
                              f64_to_wei(amount_out_f64, weth_decimals as u32)? // Use new helper
                         }
                    };
                     let final_decimals = if token_in == weth_address { weth_decimals } else { usdc_decimals };
                    println!("    Simulated Swap 2 Output ({}): {} ({})",
                         if token_in == weth_address {"WETH"} else {"USDC"},
                         amount_out_final_wei,
                         format_units(amount_out_final_wei, final_decimals as u32)?
                    );

                    // Estimate Gas Cost (Placeholder)
                    let gas_price = provider.get_gas_price().await?;
                    let estimated_gas_units = U256::from(300_000);
                    let gas_cost_wei = gas_price * estimated_gas_units;
                    println!("    Estimated Gas Cost: {} Wei ({:.8} ETH at current price)", gas_cost_wei, format_units(gas_cost_wei, "ether")?);

                    // Calculate Fees & Net Profit
                    let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128);
                    let fee_denominator = U256::from(10000);
                    let flash_loan_fee_wei = simulation_amount_weth_wei * fee_numerator / fee_denominator;
                    let total_cost_wei = flash_loan_fee_wei + gas_cost_wei;
                    let gross_profit_wei = amount_out_final_wei.saturating_sub(simulation_amount_weth_wei);

                    if gross_profit_wei > total_cost_wei {
                        net_profit_weth_wei = gross_profit_wei.saturating_sub(total_cost_wei);
                        is_profitable = true;
                        println!("    Simulated NET Profit: {} Wei ({:.8} WETH)", net_profit_weth_wei, format_units(net_profit_weth_wei, "ether")?);
                    } else {
                         println!("    Simulated NET Loss/Insufficient Profit: Gross Profit {} <= Total Cost {}", gross_profit_wei, total_cost_wei);
                    }

                    // Decision
                    if is_profitable {
                         println!("    >>> Simulation SUCCESSFUL - Proceeding to Execution <<<");
                         // TODO: Encode userData
                         // TODO: Send flash loan tx
                    } else {
                         println!("    >>> Simulation FAILED - Aborting Execution <<<");
                    }

                 } // End if spread > threshold
            }, // End Ok match
            (Err(e), _) => eprintln!("! Error Processing UniV3 Price: {}", e),
            (_, Err(e)) => eprintln!("! Error Processing VeloV2 Price: {}", e),
        } // End match prices
    // } // End loop (Temporarily commented out)
    Ok(()) // Added for single run test
} // End main