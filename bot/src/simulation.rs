// src/simulation.rs

// --- Imports ---
use ethers::{
    // Removed unused imports
    prelude::Middleware,
    types::{Address, U256},
};
use eyre::Result;
// Removed unused Arc

// --- Use statements for types from the shared bindings module ---
use crate::bindings::{
    VelodromeRouter,
    QuoterV2,
    quoter_v2 as quoter_v2_bindings,
    velodrome_router as velo_router_bindings,
};

// --- Simulation Function ---

/// Simulates the output amount of a swap...
// ... (Doc comments remain the same) ...
pub async fn simulate_swap<M: Middleware>( // Generic over Middleware M
    dex_type: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    velo_router: &VelodromeRouter<M>, // Generic over Middleware
    quoter: &QuoterV2<M>,           // Generic over Middleware
    is_velo_route_stable: bool,
    uni_pool_fee: u32,
) -> Result<U256> where M::Error: 'static + Send + Sync {
    println!(
        "    Simulating Swap: {} -> {} Amount: {} on {}",
        token_in, token_out, amount_in, dex_type
    );

    match dex_type {
        "UniV3" => {
            let params = quoter_v2_bindings::QuoteExactInputSingleParams {
                token_in,
                token_out,
                amount_in,
                fee: uni_pool_fee,
                sqrt_price_limit_x96: U256::zero(),
            };
            let quote_result = quoter.quote_exact_input_single(params).call().await;
            match quote_result {
                Ok(output) => {
                     println!("      -> UniV3 Quoter Result: AmountOut={}", output.0);
                     Ok(output.0)
                },
                Err(e) => {
                    eprintln!("      -> UniV3 Quoter simulation failed: {:?}", e);
                    Err(eyre::eyre!("UniV3 Quoter simulation failed: {:?}", e))
                },
            }
        }
        "VeloV2" => {
            let routes = vec![velo_router_bindings::Route {
                 from: token_in,
                 to: token_out,
                 stable: is_velo_route_stable,
                 factory: Address::zero(), // Placeholder
             }];
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
                     eprintln!("      -> VeloV2 getAmountsOut simulation failed: {:?}", e);
                     Err(eyre::eyre!("VeloV2 simulation failed: {:?}", e))
                 },
            }
        }
        _ => Err(eyre::eyre!("Unsupported DEX type for simulation: {}", dex_type)),
    }
}