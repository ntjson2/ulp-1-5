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
///
/// # Returns
/// * `Result<Bytes>`: The ABI-encoded `userData` or an error.
pub fn encode_user_data(
    pool_a_addr: Address,
    pool_b_addr: Address,
    token1_addr: Address,
    zero_for_one_a: bool,
    is_a_velo: bool,
    is_b_velo: bool,
    velo_router_addr: Address,
) -> Result<Bytes> {
    // Convert boolean flags to U256 values (1 or 0) as the Huff contract
    // likely reads these flags as full words (uint256) using calldataload.
    let zero_for_one_a_u256 = U256::from(if zero_for_one_a { 1 } else { 0 });
    let is_a_velo_u256 = U256::from(if is_a_velo { 1 } else { 0 });
    let is_b_velo_u256 = U256::from(if is_b_velo { 1 } else { 0 });

    // Use ethers::abi::encode_packed to concatenate the token representations
    // without padding between elements, matching common calldata packing.
    // Ensure the order of tokens matches the exact order the Huff contract
    // expects to read them with calldataload offsets.
    // Offsets based on previous analysis:
    // 0x00: pool_A_addr
    // 0x20: pool_B_addr
    // 0x40: token1_addr
    // 0x60: zeroForOne_A (as uint)
    // 0x80: is_A_Velo (as uint)
    // 0xA0: is_B_Velo (as uint)
    // 0xC0: velo_router_addr
    encode_packed(&[
        Token::Address(pool_a_addr),        // [0x00 - 0x1F]
        Token::Address(pool_b_addr),        // [0x20 - 0x3F]
        Token::Address(token1_addr),        // [0x40 - 0x5F]
        Token::Uint(zero_for_one_a_u256),   // [0x60 - 0x7F]
        Token::Uint(is_a_velo_u256),        // [0x80 - 0x9F]
        Token::Uint(is_b_velo_u256),        // [0xA0 - 0xBF]
        Token::Address(velo_router_addr),   // [0xC0 - 0xDF]
    ])
    // Map potential encoding errors to eyre::Report
    .map_err(|e| eyre::eyre!("Failed to encode user data: {}", e))
    // encode_packed returns Vec<u8>, convert it to ethers::types::Bytes
    .map(Bytes::from)
}