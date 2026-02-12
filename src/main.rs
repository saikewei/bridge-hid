use bridge_hid::core;
use bridge_hid::logging::init;
use bridge_hid::web;
use clap::{Parser, ValueEnum};
use log::{debug, info};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// 运行模式: switcher | web-touchpad
    #[arg(long, value_enum, default_value = "switcher")]
    mode: Mode,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    Switcher,
    WebTouchpad,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    init();
    let args = Args::parse();

    debug!("启动模式: {:?}", args.mode);
    match args.mode {
        Mode::Switcher => run_switcher().await?,
        Mode::WebTouchpad => run_web_touchpad().await?,
    }
    Ok(())
}

async fn run_switcher() -> anyhow::Result<()> {
    let core = core::Core::new();
    core.run().await?;

    Ok(())
}

async fn run_web_touchpad() -> anyhow::Result<()> {
    let app = web::router::build_router();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
