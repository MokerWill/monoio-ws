use core::str;

use http::Uri;
use monoio_ws::Config;

#[monoio::main]
async fn main() -> anyhow::Result<()> {
    let uri = Uri::from_static("wss://echo.websocket.org");
    let mut ws = monoio_ws::Client::connect_tls(&uri, &Config::default()).await?;

    println!("Receiving welcome text.");
    let frame = ws.read_frame().await?;
    println!(
        "Received welcome text: {frame:?} ({})",
        str::from_utf8(frame.data)?
    );

    println!("Sending ping.");
    ws.send_ping(&[]).await?;
    println!("Sent ping.");

    println!("Receiving pong.");
    let frame = ws.read_frame().await?;
    println!("Received pong: {frame:?}");

    println!("Sending text.");
    ws.send_text(b"hello").await?;
    println!("Sent text.");

    println!("Receiving text.");
    let frame = ws.read_frame().await?;
    println!("Received text: {frame:?} ({})", str::from_utf8(frame.data)?);

    Ok(())
}
