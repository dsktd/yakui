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
    /// Share this space over iroh: print its ticket, serve it at `/s/<space>/ticket`.
    #[cfg(feature = "iroh")]
    #[arg(long)]
    share: Option<host::space::SpaceId>,
    /// Join the shared space of this ticket as the `--share` space.
    #[cfg(feature = "iroh")]
    #[arg(long, requires = "share")]
    join: Option<host::iroh_bus::Ticket>,
    /// Dial the ticket's direct addresses only, no iroh relays: one LAN or tests.
    #[cfg(feature = "iroh")]
    #[arg(long, requires = "share")]
    no_relays: bool,
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
    #[cfg(feature = "iroh")]
    if let Some(space) = cli.share {
        let relays = match cli.no_relays {
            true => host::iroh_bus::Relays::Off,
            false => host::iroh_bus::Relays::Default,
        };
        let ticket = host.share(&space, cli.join, relays).await?;
        println!("{ticket}");
    }
    server::serve(host, cli.port).await
}
