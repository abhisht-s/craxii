#[cfg(all(feature = "test-failpoints", not(debug_assertions)))]
compile_error!("test-failpoints cannot be enabled when debug assertions are disabled");

pub mod adapters;
pub mod application;
pub mod bootstrap;
pub mod domain;
pub mod ports;

#[cfg(feature = "test-failpoints")]
pub mod test_failpoints;
