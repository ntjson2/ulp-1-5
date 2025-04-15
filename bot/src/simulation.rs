// src/simulation.rs

// --- Imports ---
use ethers::{
    prelude::{Provider, Http}, // Core ethers types
    types::{Address, U256}, // Address and large integer types
};
use eyre::Result; // Error handling

// --- Use statements for types from the shared bindings module ---
// Import the contract instance types and necessary structs (like Route)
use crate::bindings::{
    VelodromeRouter, // The Router type itself
    QuoterV2,        // The Quoter type itself
    quoter_v2 as quoter_v2_bindings, // Alias for accessing Quoter structs/enums
    velodrome_router as velo_router_bindings, // Alias for accessing Router structs/enums
};

// --- Simulation Function ---

/// Simulates the output amount of a swap on either Uniswap V3 or Velodrome V2
/// by calling the appropriate on-chain view function (Quoter or Router).
// ... (Doc comments remain the same) ...
pub async fn simulate_swap(
    dex_type: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    // Function now expects the types defined in src/bindings.rs
    velo_router: &VelodromeRouter<Provider<Http>>,
    quoter: &QuoterV2<Provider<Http>>,
    is_velo_route_stable: bool,
    uni_pool_fee: u32,
) -> Result<U256> {
    println!(
        "    Simulating Swap: {} -> {} Amount: {} on {}",
        token_in, token_out, amount_in, dex_type
    );

    match dex_type {
        "UniV3" => {
            // Use the imported bindings path for the params struct
            let params = quoter_v2_bindings::QuoteExactInputSingleParams {
                token_in,
                token_out,
                amount_in,
                fee: uni_pool_fee,
                sqrt_price_limit_x96: U256::zero(),
            };
            // Call method on the QuoterV2 instance passed in
            let quote_result = quoter.quote_exact_input_single(params).call().await;
            match quote_result {
                Ok(output) => {
                     println!("      -> UniV3 Quoter Result: AmountOut={}", output.0);
                     Ok(output.0)
                },
                Err(e) => {
                    eprintln!("      -> UniV3 Quoter simulation failed: {}", e);
                    Err(eyre::eyre!("UniV3 Quoter simulation failed: {}", e))
                },
            }
        }
        "VeloV2" => {
            // Use the imported bindings path for the Route struct
            let routes = vec![velo_router_bindings::Route {
                 from: token_in,
                 to: token_out,
                 stable: is_velo_route_stable,
                 factory: Address::zero(), // Still using placeholder factory
             }];
            // Call method on the VelodromeRouter instance passed in
            match velo_router.get_amounts_out(amount_in, routes).call().await {
                Ok(amounts_out) => {
                     if amounts_out.len() >= 2 {
                          println!("      -> VeloV2 getAmountsOut Result: AmountOut={}", amounts_out[1]);
                         Ok(amounts_out[1])
                     } else {
                         eprintln!("      -> VeloV2 getAmountsOut returned unexpected vector length: {:?}", amounts_out);
                         Err(eyre::eyre!("VeloV2 getAmountsOut returned unexpected vector length"))
                     }
                },
                Err(e) => {
                     eprintln!("      -> VeloV2 getAmountsOut simulation failed: {}", e);
                     Err(eyre::eyre!("VeloV2 simulation failed: {}", e))
                 },
            }
        }
        _ => Err(eyre::eyre!("Unsupported DEX type for simulation: {}", dex_type)),
    }
}