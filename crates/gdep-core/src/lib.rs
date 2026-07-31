//! Core data model, discovery, and analysis for gdep.
//!
//! This crate knows nothing about rendering or about any specific language. It reads
//! files, builds graphs, and produces a [`Report`](report::Report). Both the CLI and
//! the MCP server are adapters over it — see `design/README.md`, principle 4.

pub mod analysis;
pub mod discovery;
pub mod graph;
pub mod model;
pub mod pipeline;
pub mod provider;
pub mod report;

pub use model::{DeclaredDep, DepKind, Language, Manifest, Project, Provenance, ResolvedDep};
pub use provider::{
    Import, ImportForm, ImportTarget, LanguageProvider, ProjectContext, VersionOps,
};
pub use report::{
    CheckId, CheckStatus, Confidence, Evidence, Finding, ProjectReport, Report, SCHEMA_VERSION,
    Severity, SkippedFile,
};
