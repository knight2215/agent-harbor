//! `mcp-client` crate: MCP transports, lifecycle, and tool discovery (placeholder).
//!
//! Phase 0 scaffold. The real client, transports, and tool schema mapping land
//! in later phases.

pub mod session;
pub mod tools;

pub mod transport {
    //! MCP transport implementations (placeholders).
    pub mod http_sse;
    pub mod stdio;
}

/// Placeholder marker so dependents can reference an item from this crate,
/// proving the dependency edge compiles. Replaced with real API in later phases.
pub fn placeholder() {}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Exercises the placeholder marker so the smoke test does real work.
        super::placeholder();
    }
}
