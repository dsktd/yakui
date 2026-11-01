//! A window around the host's URL. Run the host first: `host --port 7777`.

use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::args().nth(1).unwrap_or("7777".to_owned());
    let url = format!("http://127.0.0.1:{port}/").parse()?;
    tauri::Builder::default()
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("yakui")
                .inner_size(1200.0, 800.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
