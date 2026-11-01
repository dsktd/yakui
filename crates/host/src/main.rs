use std::path::PathBuf;

use clap::Parser;
use host::Result;
use host::host::Host;
use host::registry::Registry;
use host::server;

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value_t = 7777)]
    port: u16,
    /// Layout file and pane storage; default: the user's data dir + yakui.
    #[arg(long)]
    dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    let dir = match cli.dir {
        Some(dir) => dir,
        None => dirs::data_dir().unwrap().join("yakui"),
    };
    let mut registry = Registry::default();
    registry.add::<pane_clock::Clock>();
    registry.add::<pane_inspector::Inspector>();
    registry.add::<pane_notes::Notes>();
    registry.add::<pane_outliner::Outliner>();
    registry.add::<pane_palette::Palette>();
    registry.add::<pane_view2d::View2d>();
    registry.add::<pane_view3d::View3d>();
    registry.add::<pane_widgets::Widgets>();
    let host = Host::open(dir, registry)?;
    server::serve(host, cli.port).await
}
