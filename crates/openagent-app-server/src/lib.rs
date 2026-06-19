//! App Bridge server crate for the Rust rewrite.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn core_crate_name() -> &'static str {
    openagent_core::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_core_crate() {
        assert_eq!(crate_name(), "openagent-app-server");
        assert_eq!(core_crate_name(), "openagent-core");
    }
}
