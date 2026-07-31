use regex::Regex;

fn main() -> anyhow::Result<()> {
    // Reaching into the sibling surface instead of going through `engine`.
    let digits = Regex::new(r"\d+")?;
    let input = "41";
    if digits.is_match(input) {
        println!("{}", service::handle(input)?);
    }
    Ok(())
}
