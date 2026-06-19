//! CLI crate for the Rust rewrite.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent"
}

#[must_use]
pub fn core_crate_name() -> &'static str {
    openagent_core::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-cli");
        assert_eq!(command_name(), "openagent");
        assert_eq!(core_crate_name(), "openagent-core");
    }
}
