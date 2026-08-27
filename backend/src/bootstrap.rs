use crate::application::ApplicationShell;

pub mod compatibility;
pub mod config;
pub mod credential;

pub fn build_application() -> ApplicationShell {
    ApplicationShell
}
