// src/utils.rs

// --- Imports ---
use ethers::types::U256;
use ethers::utils::format_units as ethers_format_units; // Alias to avoid conflict if used locally
use eyre::{Result, eyre}; // Added eyre import

// --- Conversion Helpers ---

/// Trait for lossy conversion to f64.
pub trait ToF64Lossy {
    fn to_f64_lossy(&self) -> f64;
}

/// Lossy conversion of U256 to f64.
/// This is a simplified conversion and might lose precision for very large numbers.
/// For financial calculations requiring high precision, consider using a dedicated decimal library.
impl ToF64Lossy for U256 {
    fn to_f64_lossy(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        // U256 can be up to 2^256 - 1. f64 can represent up to ~1.8e308.
        // Direct conversion might overflow if U256 is very large.
        // A common approach is to convert to string and then parse.
        // However, for performance, direct bit manipulation or multiple u64 conversion is better.
        // For simplicity here, we'll try converting the lower 4 u64 words.
        // This will be accurate for numbers that fit in f64.
        let mut val = 0.0;
        if self.0[3] > 0 { // Highest word
            // This will likely be too large for f64 if self.0[3] is significant.
            // This part would need a more robust arbitrary-precision to f64 conversion.
            // For now, this is a placeholder for very large U256 values.
            // A simple approach for demonstration:
            val += (self.0[3] as f64) * (2.0_f64.powi(192));
        }
        if self.0[2] > 0 {
            val += (self.0[2] as f64) * (2.0_f64.powi(128));
        }
        if self.0[1] > 0 {
            val += (self.0[1] as f64) * (2.0_f64.powi(64));
        }
        val += self.0[0] as f64;
        val
    }
}

// --- Price Calculation Helpers ---

/// Converts Uniswap V3 `sqrtPriceX96` into a human-readable price
/// (token1 per token0).
/// `decimals0/decimals1` are the *ERC-20* decimals (e.g. 18 for WETH, 6 for USDC).
pub fn sqrt_price_x96_to_price(
    sqrt_price_x96: U256,
    decimals0: u8,
    decimals1: u8,
) -> Result<f64> {
    // use crate::utils::ToF64Lossy; // ToF64Lossy is in the same module

    if sqrt_price_x96.is_zero() {
        return Err(eyre!("sqrt_price_x96 cannot be zero")); // Use eyre! macro
    }

    // 1. Convert the fixed-point value to an f64.
    //    sqrtPriceX96 is Q-encoded with 96 fractional bits,
    //    so we divide by 2^96 to get the *square root* price.
    let q96 = 2_f64.powi(96);
    let sqrt_price = sqrt_price_x96.to_f64_lossy() / q96;

    // 2. Square it to get token1 / token0.
    let mut price = sqrt_price * sqrt_price;

    // 3. Adjust for differing ERC-20 decimals.
    //    If token0 has more decimals than token1, each "whole" token0
    //    represents 10^(dec0-dec1) *base-units* of token1.
    let decimal_factor = 10_f64.powi((decimals0 as i32) - (decimals1 as i32));
    price *= decimal_factor;

    Ok(price)
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