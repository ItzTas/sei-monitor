pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn user_agent() -> String {
    format!("sei-monitor/{VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_crate_version() {
        assert_eq!(VERSION, "2.5.0");
        assert_eq!(user_agent(), "sei-monitor/2.5.0");
    }
}
