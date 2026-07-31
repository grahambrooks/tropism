pub fn handle(source: &str) -> anyhow::Result<i64> {
    engine::run(source)
}
