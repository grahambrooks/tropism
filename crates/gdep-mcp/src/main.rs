//! MCP server for gdep.
//!
//! Placeholder. The server is built at step 6 of the build order in
//! `design/07-open-questions.md`, deliberately after the CLI has proven the analysis
//! core, so the tool surface is designed against behaviour that already works.
//!
//! The tool surface itself is specified in `design/05-interfaces.md`. The governing
//! constraint: the consumer is an agent with a limited context window, so tools are
//! narrow, filtered, and paginated, and `gdep_summary` is the cheap entry point.

fn main() -> std::process::ExitCode {
    eprintln!(
        "gdep-mcp is not implemented yet (build-order step 6).\n\
         The tool surface is specified in design/05-interfaces.md.\n\
         Schema version the server will speak: {}",
        gdep_core::report::SCHEMA_VERSION
    );
    std::process::ExitCode::from(2)
}
