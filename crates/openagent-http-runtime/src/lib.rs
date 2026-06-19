//! HTTP runtime service crate for the Rust rewrite.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-http-runtime"
}

#[must_use]
pub fn app_server_crate_name() -> &'static str {
    openagent_app_server::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-http-runtime");
        assert_eq!(command_name(), "openagent-http-runtime");
        assert_eq!(app_server_crate_name(), "openagent-app-server");
    }
}
