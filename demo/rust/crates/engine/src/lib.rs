pub mod evaluator;
pub mod parser;

pub use parser::Expr;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("bad input")]
    BadInput,
}

pub fn run(source: &str) -> anyhow::Result<i64> {
    let expr = parser::parse(source)?;
    Ok(evaluator::eval(&expr))
}
