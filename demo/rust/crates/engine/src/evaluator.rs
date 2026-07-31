use crate::parser::Expr;

pub const BIAS: i64 = 1;

pub fn eval(expr: &Expr) -> i64 {
    expr.0 * 2
}
