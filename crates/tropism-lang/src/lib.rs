//! Language provider implementations.
//!
//! Each language is feature-gated so a consumer can build a smaller binary. Adding
//! the eleventh language should mean writing one module here and adding it to
//! [`registry`], touching no analyzer.

use tropism_core::provider::LanguageProvider;

#[cfg(feature = "go")]
pub mod go;

#[cfg(feature = "javascript")]
pub mod javascript;

#[cfg(feature = "rust")]
pub mod rust;

#[cfg(feature = "csharp")]
pub mod csharp;

/// Every enabled provider.
///
/// Ordering is stable so discovery output does not depend on it.
// Each push is feature-gated, so a `vec![]` literal cannot express this.
#[allow(clippy::vec_init_then_push)]
pub fn registry() -> Vec<&'static dyn LanguageProvider> {
    #[allow(unused_mut)]
    let mut providers: Vec<&'static dyn LanguageProvider> = Vec::new();

    #[cfg(feature = "go")]
    providers.push(&go::GoProvider);

    #[cfg(feature = "javascript")]
    providers.push(&javascript::JavaScriptProvider);

    #[cfg(feature = "rust")]
    providers.push(&rust::RustProvider);

    #[cfg(feature = "csharp")]
    providers.push(&csharp::CSharpProvider);

    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_not_empty_with_default_features() {
        assert!(!registry().is_empty());
    }

    #[test]
    fn no_two_providers_claim_the_same_manifest_for_the_same_language() {
        let providers = registry();
        let mut seen = Vec::new();
        for provider in &providers {
            for name in provider.manifest_names() {
                let key = (provider.language(), *name);
                assert!(!seen.contains(&key), "duplicate manifest claim: {key:?}");
                seen.push(key);
            }
        }
    }
}
