// bot/src/bindings.rs
#![allow(clippy::all)]
use ethers::prelude::abigen;

abigen!(
    UniswapV3Pool,
    "./abis/UniswapV3Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    VelodromeV2Pool,
    "./abis/VelodromeV2Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    VelodromeRouter,
    "./abis/VelodromeRouter.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    BalancerVault,
    "./abis/BalancerVault.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    QuoterV2,
    "./abis/QuoterV2.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

// Fix: Provide the correct inline ABI string for needed functions
abigen!(
    IERC20,
    r#"[
        event Approval(address indexed owner, address indexed spender, uint256 value)
        event Transfer(address indexed from, address indexed to, uint256 value)
        function approve(address spender, uint256 amount) external returns (bool)
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
        function symbol() external view returns (string)
        function name() external view returns (string)
        function totalSupply() external view returns (uint256)
        function allowance(address owner, address spender) external view returns (uint256)
        function transfer(address to, uint256 amount) external returns (bool)
        function transferFrom(address from, address to, uint256 amount) external returns (bool)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    ArbitrageExecutor,
    "./abis/ArbitrageExecutor.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    IUniswapV3Factory,
    "./abis/IUniswapV3Factory.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
// This still relies on the user manually fixing the JSON syntax error
abigen!(
    IVelodromeFactory,
    "./abis/IVelodromeFactory.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

// END OF FILE: bot/src/bindings.rs