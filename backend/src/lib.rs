#[cfg(all(feature = "test-failpoints", not(debug_assertions)))]
compile_error!("test-failpoints cannot be enabled when debug assertions are disabled");

pub mod adapters;
pub mod application;
pub mod bootstrap;
pub mod distribution_contracts;
pub mod domain;
pub mod ports;
pub mod protocol;

#[cfg(feature = "test-failpoints")]
pub mod test_failpoints;

pub mod worker_auth;

pub mod distribution;

pub mod guest_boundary;
