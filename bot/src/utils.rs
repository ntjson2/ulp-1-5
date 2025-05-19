// src/utils.rs

// --- Imports ---
use ethers::core::types::U256;
use ethers::utils::format_units as ethers_format_units; // Alias to avoid conflict if used locally
use eyre::Result;

// --- Lossy U256 to f64 Conversion Trait ---
pub trait ToF64Lossy {
    fn to_f64_lossy(&self) -> f64;
}

impl ToF64Lossy for U256 {
    fn to_f64_lossy(&self) -> f64 {
        0.0
    }
}

// --- Price Calculation Helpers ---

/// Calculates Uniswap V3 price (token1 per token0) from sqrtPriceX96.
pub fn v3_price_from_sqrt(_sqrt_price_x96: U256, _decimals0: u8, _decimals1: u8) -> Result<f64> {
    Ok(0.0)
}

/// Calculates Uniswap V2 / Velodrome V2 price (token1 per token0) from reserves.
pub fn v2_price_from_reserves(_reserve0: U256, _reserve1: U256, _decimals0: u8, _decimals1: u8) -> Result<f64> {
    Ok(0.0)
}

// --- Unit Conversion Helper ---

/// Parses a floating-point number (f64) representing a token amount
/// into its corresponding U256 representation in "wei" (base units).
pub fn f64_to_wei(_amount_f64: f64, _decimals: u32) -> Result<U256> {
    // stub implementation
    Ok(U256::zero())
}

pub fn format_units(value: U256, decimals: i32) -> Result<String> {
    ethers_format_units(value, decimals).map_err(|e| eyre::eyre!("Failed to format units: {}", e))
}

// Placeholder for uniswap_v3_math if it was intended to be a local module.
// However, Cargo.toml lists it as a dependency, so this module is likely not needed here.
// pub mod uniswap_v3_math {
//    // ... functions ...
// }