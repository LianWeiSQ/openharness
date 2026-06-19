//! Agent-agnostic swarm kernel crate for the Rust rewrite.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-swarm"
}

#[must_use]
pub fn protocol_crate_name() -> &'static str {
    openagent_protocol::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-swarm");
        assert_eq!(command_name(), "openagent-swarm");
        assert_eq!(protocol_crate_name(), "openagent-protocol");
    }
}
