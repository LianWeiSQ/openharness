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
