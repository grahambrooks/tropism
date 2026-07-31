//! Output renderers. All are pure functions of a finished `Report` — no analysis
//! logic lives here.
//!
//! JSON is deliberately absent: it is produced by `Report::to_json_pretty` in
//! `tropism-core`, so the CLI and the MCP server serialize through one implementation
//! rather than two that can drift.

pub mod text;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(test)]
pub mod testdata;
