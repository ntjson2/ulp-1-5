// bot/src/lib.rs
// New file to define the library interface

// Import EthEvent trait
use ethers::types::H256; // Keep this one

// Re-export modules needed by tests and binary
pub mod bindings;
pub mod config;
pub mod deploy;
pub mod encoding;
pub mod event_handler;
pub mod event_loop;
pub mod gas;
pub mod path_optimizer;
pub mod simulation;
pub mod state;
pub mod transaction;
pub mod utils;

// Re-export specific items for convenience
pub use config::Config;
pub use state::AppState;

// Topics (ensure these are defined within lib.rs or a module it owns)
pub const UNI_V3_POOL_CREATED_TOPIC: H256 = H256([
    0x78, 0x3a, 0xdd, 0x42, 0x67, 0xe6, 0xdc, 0xb9, 0xcb, 0x67, 0x90, 0x36, 0x11, 0x1a, 0x4c, 0xb0,
    0x6f, 0x43, 0xad, 0x3d, 0x76, 0x2f, 0x18, 0x09, 0xad, 0xab, 0x4d, 0x63, 0xef, 0xf0, 0xae, 0x7c,
]);
pub const UNI_V3_SWAP_TOPIC: H256 = H256([
    0xc4, 0x20, 0x79, 0x1a, 0xcc, 0x4a, 0x61, 0x76, 0x43, 0xed, 0x93, 0x12, 0x06, 0x2f, 0x58, 0x45,
    0x54, 0xf0, 0xda, 0x3e, 0xa4, 0x08, 0xdd, 0x49, 0x89, 0x0e, 0x2c, 0x44, 0x83, 0x8d, 0x81, 0xd4,
]);
// Placeholder - replace with actual Velo/Aero PoolCreated topic if different
pub const VELO_AERO_POOL_CREATED_TOPIC: H256 = H256([
    0x3c, 0x8c, 0x01, 0x29, 0x38, 0x09, 0x64, 0x31, 0x18, 0xff, 0x68, 0x58, 0x89, 0x1f, 0xcb, 0xa4,
    0x70, 0xa1, 0xa3, 0xa1, 0xd6, 0x33, 0x54, 0x6a, 0xcc, 0xa5, 0x51, 0x59, 0x40, 0x75, 0x8b, 0x0a,
]);
// Using UniV3 Swap topic as a placeholder for Velo/Aero if they share it or until specific one is found
pub const VELO_AERO_SWAP_TOPIC: H256 = UNI_V3_SWAP_TOPIC;


// Constants are defined in simulation.rs and re-exported here.
// Do NOT redefine them here.
// pub const VELO_ROUTER_IMPL_ADDR_FOR_SIM: &str = "0x0000000000000000000000000000000000000000";
// pub const PAIR_DOES_NOT_EXIST_SELECTOR_STR: &str = "";


// Re-export simulation API for tests and main
pub use simulation::{
    SIMULATION_CONFIG,
    VELO_ROUTER_IMPL_ADDR_FOR_SIM, // This will be resolved if defined in simulation.rs
    PAIR_DOES_NOT_EXIST_SELECTOR_STR, // This will be resolved if defined in simulation.rs
};
// Legacy import path for tests if needed, or tests can use ulp1_5::simulation
pub use simulation as local_simulator;

// Re-exports for main.rs and tests
pub use deploy::deploy_contract_from_bytecode;
pub use event_handler::{handle_log_event, handle_new_block};
pub use event_loop::run_event_loop_ws_test; // Re-export from event_loop module