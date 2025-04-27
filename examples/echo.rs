use core::str;

use http::Uri;
use monoio_ws::Config;

#[monoio::main]
async fn main() -> anyhow::Result<()> {
    let uri = Uri::from_static("wss://echo.websocket.org");
    let mut ws = monoio_ws::Client::connect_tls(&uri, &Config::default()).await?;

    println!("Receiving welcome text.");
    let (res, mut buffer) = ws.read_frame(Vec::with_capacity(4096)).await;
    let frame = res?;
    println!(
        "Received welcome text: {frame:?} {}",
        str::from_utf8(&buffer)?
    );

    println!("Sending ping.");
    ws.send_ping(&[]).await?;
    println!("Sent ping.");

    println!("Receiving pong.");
    buffer.clear();
    let (res, mut buffer) = ws.read_frame(buffer).await;
    let frame = res?;
    println!("Received pong: {frame:?} {buffer:?}");

    println!("Sending text.");
    ws.send_text(b"hello").await?;
    println!("Sent text.");

    println!("Receiving text.");
    buffer.clear();
    let (res, buffer) = ws.read_frame(buffer).await;
    let frame = res?;
    println!("Received text: {frame:?} {}", str::from_utf8(&buffer)?);

    Ok(())
}
