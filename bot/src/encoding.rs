// src/encoding.rs
// Module for encoding data specific to the arbitrage strategy,
// particularly the userData for the Huff contract's flash loan callback.

use ethers::{
    abi::{encode_packed, Token}, // Functions for ABI encoding
    types::{Address, Bytes, U256}, // Core Ethereum types
};
use eyre::Result; // Error handling

/// Encodes the parameters required by the ArbitrageExecutor Huff contract's
/// `receiveFlashLoan` function into a tightly packed `Bytes` object.
///
/// The Huff contract expects parameters loaded via `calldataload` at specific offsets,
/// assuming standard 32-byte packing.
///
/// # Arguments
/// * `pool_a_addr`: Address of the first pool in the swap path (where the buy happens).
/// * `pool_b_addr`: Address of the second pool in the swap path (where the sell happens).
/// * `token1_addr`: Address of the intermediate token (e.g., USDC if loan is WETH).
/// * `zero_for_one_a`: Boolean indicating direction of Swap A (true if token0->token1).
/// * `is_a_velo`: Boolean indicating if Pool A is a Velodrome pool.
/// * `is_b_velo`: Boolean indicating if Pool B is a Velodrome pool.
/// * `velo_router_addr`: Address of the Velodrome Router (needed by Huff for Velo swaps).
/// * `min_profit_wei`: Minimum required profit in loan token (T0) wei for tx to succeed.
/// * `salt`: A unique nonce/salt (uint256) for this specific transaction attempt.
///
/// # Returns
/// * `Result<Bytes>`: The ABI-encoded `userData` or an error.
pub fn encode_user_data(
    pool_a_addr: Address,
    pool_b_addr: Address,
    token1_addr: Address, // Intermediate token
    zero_for_one_a: bool,
    is_a_velo: bool,
    is_b_velo: bool,
    velo_router_addr: Address,
    min_profit_wei: U256, // Minimum profit threshold in loan token wei
    salt: U256,           // Unique salt for replay protection
) -> Result<Bytes> {
    // Convert boolean flags to U256 values (1 or 0)
    let zero_for_one_a_u256 = U256::from(u8::from(zero_for_one_a));
    let is_a_velo_u256 = U256::from(u8::from(is_a_velo));
    let is_b_velo_u256 = U256::from(u8::from(is_b_velo));

    // Use ethers::abi::encode_packed to concatenate the token representations
    // Offsets based on Huff contract v2.3.0:
    // 0x00: pool_A_addr
    // 0x20: pool_B_addr
    // 0x40: token1_addr
    // 0x60: zeroForOne_A (as uint)
    // 0x80: is_A_Velo (as uint)
    // 0xA0: is_B_Velo (as uint)
    // 0xC0: velo_router_addr
    // 0xE0: minProfitWei (as uint)
    // 0x100: salt (as uint) ** NEW **
    encode_packed(&[
        Token::Address(pool_a_addr),        // [0x00 - 0x1F]
        Token::Address(pool_b_addr),        // [0x20 - 0x3F]
        Token::Address(token1_addr),        // [0x40 - 0x5F]
        Token::Uint(zero_for_one_a_u256),   // [0x60 - 0x7F]
        Token::Uint(is_a_velo_u256),        // [0x80 - 0x9F]
        Token::Uint(is_b_velo_u256),        // [0xA0 - 0xBF]
        Token::Address(velo_router_addr),   // [0xC0 - 0xDF]
        Token::Uint(min_profit_wei),        // [0xE0 - 0xFF]
        Token::Uint(salt),                  // [0x100 - 0x11F] ** NEW **
    ])
    .map_err(|e| eyre::eyre!("Failed to encode user data: {}", e))
    .map(Bytes::from)
}

pub fn encode_user_data() -> Vec<u8> {
    Vec::new()
}

// END OF FILE: bot/src/encoding.rs