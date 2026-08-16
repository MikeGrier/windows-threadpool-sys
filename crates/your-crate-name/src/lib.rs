//! Replace this placeholder crate with your actual library API.

/// Returns a greeting for the provided crate or project name.
#[must_use]
pub fn greeting(name: &str) -> String {
    format!("hello from {name}")
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greeting_includes_name() {
        assert_eq!(greeting("your-crate-name"), "hello from your-crate-name");
    }
}
