// src/utils.rs

// --- Imports ---
use ethers::types::U256; // For large integer types used in calculations
use eyre::{Result, WrapErr}; // For convenient error handling
use std::str::FromStr; // To convert strings to numbers (used in U256 -> f64)
use tracing::warn; // For logging warnings on potential precision loss

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
        // Consider using f64::INFINITY or a dedicated error type for clearer failure indication.
        match f64::from_str(&self.to_string()) {
            Ok(f) if f.is_finite() => f,
            Ok(_) => {
                // Value parsed but is infinite or NaN
                warn!("U256 to f64 conversion resulted in non-finite value for {}", self);
                f64::MAX // Indicate overflow or non-finite result clearly
            }
            Err(_) => {
                // Parsing from string failed, likely too large
                warn!("U256 to f64 conversion failed for {} (too large?)", self);
                f64::MAX // Indicate overflow or non-finite result clearly
            }
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
    if sqrt_price_x96_f64 == f64::MAX { // Check for conversion failure
        return Err(eyre::eyre!("sqrt_price_x96 U256->f64 conversion failed (too large? value: {})", sqrt_price_x96));
    }
    // Q96 constant (2^96) as f64
    let q96_f64: f64 = 2.0_f64.powi(96);
    // Avoid division by zero if Q96 calculation somehow fails (very unlikely)
    if q96_f64 == 0.0 {
        return Err(eyre::eyre!("Q96 constant is zero, cannot calculate price"));
    }
    // Calculate price ratio: (sqrtPriceX96 / 2^96)^2
    let price_ratio_f64 = (sqrt_price_x96_f64 / q96_f64).powi(2);
    // Calculate decimal adjustment factor: 10^(decimals0 - decimals1)
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    // Apply decimal adjustment to get the final price
    let final_price = price_ratio_f64 * adjustment;

    // Check for non-finite results (Infinity, NaN) which can occur with extreme values
    if !final_price.is_finite() {
        Err(eyre::eyre!("Calculated UniV3 price is non-finite (sqrtP: {}, ratio: {}, adj: {})", sqrt_price_x96_f64, price_ratio_f64, adjustment))
    } else {
        Ok(final_price)
    }
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
    // Handle zero reserve edge case for the denominator
    if reserve0.is_zero() {
        // If reserve1 is also zero, price is undefined (or 0). If reserve1 > 0, price is infinite.
        // Returning 0.0 might be misleading. Return error for clarity.
        return Err(eyre::eyre!("Reserve0 is zero, cannot calculate V2 price"));
    }
    // Convert reserves to f64 using our lossy helper
    let reserve0_f64 = reserve0.to_f64_lossy();
    let reserve1_f64 = reserve1.to_f64_lossy();

    if reserve0_f64 == f64::MAX || reserve1_f64 == f64::MAX { // Check conversion failure
        return Err(eyre::eyre!("Reserve U256->f64 conversion failed (r0: {}, r1: {})", reserve0, reserve1));
    }

    // Avoid division by zero (already handled by U256 check, but good practice for f64)
    if reserve0_f64.abs() < f64::EPSILON {
         return Err(eyre::eyre!("Reserve0 f64 value is near zero, cannot calculate V2 price"));
    }
    // Calculate price ratio: reserve1 / reserve0
    let price_ratio_f64 = reserve1_f64 / reserve0_f64;
    // Calculate decimal adjustment factor: 10^(decimals0 - decimals1)
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    // Apply decimal adjustment
    let final_price = price_ratio_f64 * adjustment;

    // Check for non-finite results
    if !final_price.is_finite() {
        Err(eyre::eyre!("Calculated V2 price is non-finite (r0: {}, r1: {}, ratio: {}, adj: {})", reserve0_f64, reserve1_f64, price_ratio_f64, adjustment))
    } else {
        Ok(final_price)
    }
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
    // Ensure input is a valid finite, non-negative number
    if !amount_f64.is_finite() || amount_f64 < 0.0 {
        return Err(eyre::eyre!("Cannot convert non-finite or negative f64 '{}' to U256", amount_f64));
    }
    // Calculate the multiplier (10^decimals) as f64
    let multiplier = 10f64.powi(decimals as i32);
    // Multiply the float amount by the multiplier and round to the nearest whole number
    // to get the value in wei as a float.
    let wei_f64 = (amount_f64 * multiplier).round();

    // Ensure the result is representable without loss before converting to string
    // Check if the rounded f64 is still finite and non-negative.
    // Also check if it exceeds the theoretical max value representable by f64 accurately for integers.
    // Note: f64 can accurately represent integers up to 2^53. U256::MAX is much larger.
    // This check is primarily for NaN/Infinity after rounding.
    if !wei_f64.is_finite() || wei_f64 < 0.0 {
         return Err(eyre::eyre!("Intermediate wei calculation resulted in non-finite or negative value for f64 '{}'", amount_f64));
    }

    // Convert the resulting float (which should now be a whole number)
    // to a string with zero decimal places to handle potentially very large numbers
    // that might exceed f64 precision if kept as float.
    let wei_str = format!("{:.0}", wei_f64);

    // Parse the string representation into a U256.
    U256::from_dec_str(&wei_str)
         .wrap_err_with(|| format!("Failed to parse f64 '{}' (wei string '{}', decimals {}) to U256", amount_f64, wei_str, decimals))
}
// END OF FILE: bot/src/utils.rs