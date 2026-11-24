//! `tropism portfolio`: the links between a team's own repositories.
//!
//! `tropism analyze` and `tropism check` answer questions about one repository.
//! This answers the ones that span several: who uses a library, which users are
//! behind its newest tag, whether its default branch is waiting on a release, and
//! whether a lockfile holds two copies of it. It keeps tropism's rule — files are
//! read, no package manager runs, nothing is built — and adds only read-only `git`
//! for remotes and tags, which is history rather than resolution.
//!
//! The output is a versioned JSON contract ([`model::PortfolioReport`]) meant for
//! other tools as much as for people. See `design/20-portfolio.md`.
//!
//! This crate sits beside `tropism-core` rather than inside it: core is
//! language-agnostic and knows nothing of version control, and both are true of
//! nothing here.

pub mod analyze;
pub mod binary;
pub mod cargo;
pub mod gomod;
pub mod history;
pub mod model;
pub mod npm;
pub mod remote;
pub mod rules;
pub mod scan;
mod text;
pub mod version;

pub use analyze::{Options, run};
pub use history::{Git, History};
pub use model::{PortfolioReport, SCHEMA, SCHEMA_VERSION};
pub use rules::{PortfolioRules, RULES_FILE};
