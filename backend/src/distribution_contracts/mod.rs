//! Pure, versioned distribution contracts. Parsing never authenticates a principal.
pub mod canonical;
pub mod compatibility;
pub mod primitives;
pub mod raw_json;
pub mod signing;
pub mod wire;

/// Safe bounded diagnostics only; no supplied payload or parser error is exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractError(pub &'static str);
pub type ContractResult<T> = Result<T, ContractError>;
pub(crate) fn invalid() -> ContractError {
    ContractError("invalid_contract")
}
