use serde_json::from_str;

use crate::evaluator;

pub struct Expr(pub i64);

pub fn parse(source: &str) -> anyhow::Result<Expr> {
    let raw = from_str::<i64>(source).unwrap_or(0);
    Ok(Expr(raw + evaluator::BIAS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert!(parse("1").is_ok());
    }
}
