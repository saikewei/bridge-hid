use bridge_hid::core;
use bridge_hid::logging::init;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init();
    let core = core::Core::new();
    core.run().await?;
    Ok(())
}
