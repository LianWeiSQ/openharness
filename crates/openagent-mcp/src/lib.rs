//! MCP config, auth, discovery, and tool bridge crate for the Rust rewrite.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn tool_crate_name() -> &'static str {
    openagent_tools::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_tools_crate() {
        assert_eq!(crate_name(), "openagent-mcp");
        assert_eq!(tool_crate_name(), "openagent-tools");
    }
}
