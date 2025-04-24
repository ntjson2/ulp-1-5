// src/utils.rs

// --- Imports ---
use ethers::types::U256; // For large integer types used in calculations
use eyre::Result; // For convenient error handling
use std::str::FromStr; // To convert strings to numbers (used in U256 -> f64)

// --- Lossy U256 to f64 Conversion Trait ---
// Adds a method `.to_f64_lossy()` to U256 numbers.
// This conversion is "lossy" because f64 cannot represent all possible U256 values accurately,
// especially very large ones, but it's useful for quick price comparisons/approximations.
// Marked `pub` so the trait and its method can be used in other modules.
pub trait ToF64Lossy {
    fn to_f64_lossy(&self) -> f64;
}

// Implementation of the trait for ethers' U256 type.
impl ToF64Lossy for U256 {
    fn to_f64_lossy(&self) -> f64 {
        // Convert the U256 to a string first, then parse the string as f64.
        // This handles larger numbers better than direct casting.
        // If parsing fails (e.g., number too large), return f64::MAX as an indicator.
        match f64::from_str(&self.to_string()) {
            Ok(f) => f,
            Err(_) => f64::MAX,
        }
    }
}

// --- Price Calculation Helpers ---

// Calculates Uniswap V3 price (token1 per token0) from sqrtPriceX96.
// Uses f64 math for simplicity, suitable for initial detection, not precise simulation.
// Arguments:
//   - sqrt_price_x96: The sqrtPriceX96 value from the pool's slot0.
//   - decimals0: Number of decimals for token0.
//   - decimals1: Number of decimals for token1.
// Marked `pub` for use in other modules.
pub fn v3_price_from_sqrt(sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    // Handle zero price edge case
    if sqrt_price_x96.is_zero() {
        return Ok(0.0);
    }
    // Convert U256 to f64 using our lossy helper trait
    let sqrt_price_x96_f64 = sqrt_price_x96.to_f64_lossy();
    // Q96 constant (2^96) as f64
    let q96_f64: f64 = 2.0_f64.powi(96);
    // Avoid division by zero if Q96 calculation somehow fails (very unlikely)
    if q96_f64 == 0.0 {
        return Err(eyre::eyre!("Q96 calculation resulted in zero"));
    }
    // Calculate price ratio: (sqrtPriceX96 / 2^96)^2
    let price_ratio_f64 = (sqrt_price_x96_f64 / q96_f64).powi(2);
    // Calculate decimal adjustment factor: 10^(decimals0 - decimals1)
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    // Apply decimal adjustment to get the final price
    Ok(price_ratio_f64 * adjustment)
}

// Calculates Uniswap V2 / Velodrome V2 price (token1 per token0) from reserves.
// Uses f64 math, suitable for initial detection.
// Arguments:
//   - reserve0: Reserve balance of token0.
//   - reserve1: Reserve balance of token1.
//   - decimals0: Number of decimals for token0.
//   - decimals1: Number of decimals for token1.
// Marked `pub` for use in other modules.
pub fn v2_price_from_reserves(reserve0: U256, reserve1: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    // Handle zero reserve edge case
    if reserve0.is_zero() {
        return Ok(0.0);
    }
    // Convert reserves to f64 using our lossy helper
    let reserve0_f64 = reserve0.to_f64_lossy();
    let reserve1_f64 = reserve1.to_f64_lossy();
    // Avoid division by zero
    if reserve0_f64.abs() < f64::EPSILON {
        return Ok(0.0);
    }
    // Calculate price ratio: reserve1 / reserve0
    let price_ratio_f64 = reserve1_f64 / reserve0_f64;
    // Calculate decimal adjustment factor: 10^(decimals0 - decimals1)
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    // Apply decimal adjustment
    Ok(price_ratio_f64 * adjustment)
}

// --- Unit Conversion Helper ---

// Parses a floating-point number (f64) representing a token amount
// into its corresponding U256 representation in "wei" (base units),
// given the token's decimal count.
// Arguments:
//   - amount_f64: The token amount as a float (e.g., 1.5 ETH).
//   - decimals: The number of decimals the token uses (e.g., 18 for ETH).
// Marked `pub` for use in other modules.
pub fn f64_to_wei(amount_f64: f64, decimals: u32) -> Result<U256> {
    // Ensure input is a valid finite number
    if !amount_f64.is_finite() {
        return Err(eyre::eyre!("Cannot convert non-finite f64 to U256"));
    }
    // Calculate the multiplier (10^decimals) as f64
    let multiplier = 10f64.powi(decimals as i32);
    // Multiply the float amount by the multiplier and round to the nearest whole number
    // to get the value in wei as a float.
    let wei_f64 = (amount_f64 * multiplier).round();
    // Ensure the result is not negative
    if wei_f64 < 0.0 {
         return Err(eyre::eyre!("Cannot convert negative f64 to U256"));
    }
    // Convert the resulting float (which should now be a whole number)
    // to a string with zero decimal places to handle potentially very large numbers
    // that might exceed f64 precision if kept as float.
    let wei_str = format!("{:.0}", wei_f64);
    // Parse the string representation into a U256.
    U256::from_dec_str(&wei_str)
         .map_err(|e| eyre::eyre!("Failed to parse f64 '{}' (wei string '{}') to U256: {}", amount_f64, wei_str, e))
}