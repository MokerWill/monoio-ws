use core::str;

use http::Uri;

#[monoio::main]
async fn main() -> anyhow::Result<()> {
    let uri = Uri::from_static("wss://echo.websocket.org");
    let mut ws = monoio_ws::Client::connect_tls(&uri).await?;

    let mut buffer = Vec::with_capacity(4096);

    println!("Receiving welcome text.");
    buffer.clear();
    let (res, mut buffer) = ws.read_frame(buffer).await;
    let frame = res?;
    println!(
        "Received welcome text: {frame:?} {}",
        str::from_utf8(&buffer)?
    );

    println!("Sending ping.");
    buffer.clear();
    let (res, mut buffer) = ws.send_ping(buffer).await;
    res?;
    println!("Sent ping.");

    println!("Receiving pong.");
    buffer.clear();
    let (res, mut buffer) = ws.read_frame(buffer).await;
    let frame = res?;
    println!("Received pong: {frame:?} {buffer:?}");

    println!("Sending text.");
    buffer.clear();
    buffer.extend_from_slice(b"hello");
    let (res, mut buffer) = ws.send_text(buffer).await;
    res?;
    println!("Sent text.");

    println!("Receiving text.");
    buffer.clear();
    let (res, buffer) = ws.read_frame(buffer).await;
    let frame = res?;
    println!("Received text: {frame:?} {}", str::from_utf8(&buffer)?);

    Ok(())
}
