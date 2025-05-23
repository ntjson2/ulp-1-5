// bot/src/lib.rs
// New file to define the library interface

// Import EthEvent trait
use ethers::contract::EthEvent; // <-- Added this line
use ethers::types::H256;
use lazy_static::lazy_static;

// Re-export modules needed by integration tests and potentially the binary
pub mod bindings;
pub mod config;
pub mod deploy;
pub mod encoding;
pub mod event_handler;
pub mod gas;
#[cfg(feature = "local_simulation")] // Conditionally compile local_simulator
pub mod local_simulator;
pub mod path_optimizer;
pub mod simulation;
pub mod state;
pub mod transaction;
pub mod utils;

// Public types/constants re-exported for convenience
pub use state::{AppState, DexType, PoolSnapshot, PoolState}; // Re-export key types
pub use path_optimizer::RouteCandidate;
pub use transaction::NonceManager; // Re-export NonceManager

// Re-export event topics if needed directly by tests/binary

// Define lazy_static within the lib so they are accessible via the library.
lazy_static! {
    pub static ref UNI_V3_SWAP_TOPIC: H256 = bindings::uniswap_v3_pool::SwapFilter::signature();
    pub static ref UNI_V3_POOL_CREATED_TOPIC: H256 = bindings::i_uniswap_v3_factory::PoolCreatedFilter::signature();
    pub static ref VELO_AERO_SWAP_TOPIC: H256 = bindings::velodrome_v2_pool::SwapFilter::signature();
    pub static ref VELO_AERO_POOL_CREATED_TOPIC: H256 = bindings::i_velodrome_factory::PoolCreatedFilter::signature();
}