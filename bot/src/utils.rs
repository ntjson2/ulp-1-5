// src/utils.rs

// --- Imports ---
use ethers::types::U256;

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
    Ok(U256::zero())
}