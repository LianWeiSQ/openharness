#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_protocol_crate() {
        assert_eq!(crate_name(), "openagent-session");
        assert_eq!(protocol_crate_name(), "openagent-protocol");
    }
}
