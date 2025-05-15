// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.19;

import "forge-std/console.sol"; // Optional for debugging in Remix/Hardhat

// Minimal IERC20 interface for token addresses in event
interface IERC20 {
    function decimals() external view returns (uint8);
    function symbol() external view returns (string memory);
}

contract MinimalSwapEmitter {
    // Matching the UniswapV3Pool Swap event 
    event Swap(
        address indexed sender,
        address indexed recipient,
        int256 amount0,
        int256 amount1,
        uint160 sqrtPriceX96,
        uint128 liquidity,
        int24 tick
    );

    // We will make this contract emit the event AS IF it were the target pool.
    // The 'sender' and 'recipient' for the event will be this contract's address
    // or can be passed in if more control is needed.
    // For simplicity, msg.sender (the EOA calling this) will be the event's sender and recipient.

    function emitMinimalSwap(
        int256 amount0,
        int256 amount1,
        uint160 sqrtPriceX96,
        uint128 liquidity,
        int24 tick
    ) external {
        // console.log("MinimalSwapEmitter: Emitting Swap event...");
        // console.log("  sender:", msg.sender);
        // console.log("  recipient:", msg.sender);
        // console.log("  amount0:", amount0);
        // console.log("  amount1:", amount1);
        // console.log("  sqrtPriceX96:", sqrtPriceX96);
        // console.log("  liquidity:", liquidity);
        // console.log("  tick:", tick);
        emit Swap(
            msg.sender,       // The EOA calling this function
            msg.sender,       // The EOA calling this function
            amount0,
            amount1,
            sqrtPriceX96,
            liquidity,
            tick
        );
    }
}