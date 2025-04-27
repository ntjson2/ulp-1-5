// src/utils.rs

// --- Imports ---
use ethers::types::{Address, U256}; // Added Address import
use eyre::{Result, WrapErr}; // For convenient error handling
use std::str::FromStr; // To convert strings to numbers (used in U256 -> f64)
use tracing::{instrument, warn}; // Added instrument macro

// --- Lossy U256 to f64 Conversion Trait ---
pub trait ToF64Lossy {
    fn to_f64_lossy(&self) -> f64;
}

impl ToF64Lossy for U256 {
    // Add instrument for tracing potential conversion issues
    #[instrument(level="trace", fields(num = %self))]
    fn to_f64_lossy(&self) -> f64 {
        match f64::from_str(&self.to_string()) {
            Ok(f) if f.is_finite() => f,
            Ok(_) => {
                warn!("U256 to f64 conversion resulted in non-finite value");
                f64::MAX // Indicate overflow or non-finite result clearly
            }
            Err(_) => {
                warn!("U256 to f64 conversion failed (too large?)");
                f64::MAX // Indicate overflow or non-finite result clearly
            }
        }
    }
}

// --- Price Calculation Helpers ---

/// Calculates Uniswap V3 price (token1 per token0) from sqrtPriceX96.
#[instrument(level="trace")]
pub fn v3_price_from_sqrt(sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if sqrt_price_x96.is_zero() {
        return Ok(0.0);
    }
    let sqrt_price_x96_f64 = sqrt_price_x96.to_f64_lossy();
    if sqrt_price_x96_f64 == f64::MAX {
        return Err(eyre::eyre!("sqrt_price_x96 U256->f64 conversion failed"));
    }
    let q96_f64: f64 = (U256::one() << 96).to_f64_lossy(); // Calculate Q96 using U256
    if q96_f64 == 0.0 || q96_f64 == f64::MAX { // Check Q96 conversion
        return Err(eyre::eyre!("Q96 constant conversion failed"));
    }

    let price_ratio_f64 = (sqrt_price_x96_f64 / q96_f64).powi(2);
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    let final_price = price_ratio_f64 * adjustment;

    if !final_price.is_finite() {
        Err(eyre::eyre!("Calculated UniV3 price is non-finite (ratio: {}, adj: {})", price_ratio_f64, adjustment))
    } else {
        Ok(final_price)
    }
}

/// Calculates Uniswap V2 / Velodrome V2 price (token1 per token0) from reserves.
#[instrument(level="trace")]
pub fn v2_price_from_reserves(reserve0: U256, reserve1: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if reserve0.is_zero() {
        return Err(eyre::eyre!("Reserve0 is zero, cannot calculate V2 price"));
    }
    let reserve0_f64 = reserve0.to_f64_lossy();
    let reserve1_f64 = reserve1.to_f64_lossy();

    if reserve0_f64 == f64::MAX || reserve1_f64 == f64::MAX {
        return Err(eyre::eyre!("Reserve U256->f64 conversion failed"));
    }

    if reserve0_f64.abs() < f64::EPSILON {
         return Err(eyre::eyre!("Reserve0 f64 value is near zero, cannot calculate V2 price"));
    }

    let price_ratio_f64 = reserve1_f64 / reserve0_f64;
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    let final_price = price_ratio_f64 * adjustment;

    if !final_price.is_finite() {
        Err(eyre::eyre!("Calculated V2 price is non-finite (ratio: {}, adj: {})", price_ratio_f64, adjustment))
    } else {
        Ok(final_price)
    }
}

// --- Unit Conversion Helper ---

/// Parses a floating-point number (f64) representing a token amount
/// into its corresponding U256 representation in "wei" (base units).
#[instrument(level="trace")]
pub fn f64_to_wei(amount_f64: f64, decimals: u32) -> Result<U256> {
    if !amount_f64.is_finite() || amount_f64 < 0.0 {
        return Err(eyre::eyre!("Cannot convert non-finite or negative f64 '{}' to U256", amount_f64));
    }
    let multiplier = 10f64.powi(decimals as i32);
    let wei_f64 = (amount_f64 * multiplier).round();

    if !wei_f64.is_finite() || wei_f64 < 0.0 {
         return Err(eyre::eyre!("Intermediate wei calculation resulted in non-finite or negative value for f64 '{}'", amount_f64));
    }

    let wei_str = format!("{:.0}", wei_f64);

    U256::from_dec_str(&wei_str)
         .wrap_err_with(|| format!("Failed to parse f64 '{}' (wei string '{}', decimals {}) to U256", amount_f64, wei_str, decimals))
}

// END OF FILE: bot/src/utils.rs