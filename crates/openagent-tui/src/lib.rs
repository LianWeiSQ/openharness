//! Terminal UI crate for the Rust rewrite.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-tui"
}

#[must_use]
pub fn client_crate_name() -> &'static str {
    openagent_app_server_client::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-tui");
        assert_eq!(command_name(), "openagent-tui");
        assert_eq!(client_crate_name(), "openagent-app-server-client");
    }
}
