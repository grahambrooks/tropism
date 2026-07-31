fn main() -> anyhow::Result<()> {
    println!("{}", engine::run("41")?);
    Ok(())
}
