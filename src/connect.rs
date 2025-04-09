use std::sync::Arc;

use base64::{Engine, prelude::BASE64_STANDARD};
use http::Uri;
use monoio::{
    io::{AsyncBufReadExt, AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt, BufReader},
    net::TcpStream,
};
use monoio_rustls::{Stream, TlsConnector, TlsError};
use rand::Rng;
use rustls::{ClientConfig, ClientConnection, pki_types::InvalidDnsNameError};
use sha1::{Digest, Sha1};

use crate::Client;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS: {0}")]
    Tls(#[from] TlsError),
    #[error("Invalid handshake response: {0}")]
    InvalidHandshakeResponse(String),
    #[error("Invalid Sec-WebSocket-Accept header")]
    InvalidWebSocketAcceptHeader,
    #[error("DNS: {0}")]
    InvalidDnsName(#[from] InvalidDnsNameError),
    #[error("Attempted to connect with invalid URI scheme")]
    InvalidUriScheme,
}

pub type ConnectResult<T> = std::result::Result<T, ConnectError>;

impl Client<BufReader<Stream<TcpStream, ClientConnection>>> {
    pub async fn connect_tls(uri: &Uri) -> ConnectResult<Self> {
        if uri.scheme_str() != Some("wss") {
            return Err(ConnectError::InvalidUriScheme);
        }

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(config));
        let server_name =
            rustls::pki_types::ServerName::try_from(uri.host().unwrap_or_default().to_string())?;

        // Connect, upgrade to TLS and perform WebSocket handshake.
        let stream = TcpStream::connect(format!(
            "{}:{}",
            uri.host().unwrap_or_default(),
            uri.port_u16().unwrap_or(443)
        ))
        .await?;

        let stream = connector.connect(server_name, stream).await?;
        Ok(Self::new(handshake(stream, uri).await?))
    }
}

impl Client<BufReader<TcpStream>> {
    pub async fn connect_plain(uri: &Uri) -> ConnectResult<Self> {
        if uri.scheme_str() != Some("ws") {
            return Err(ConnectError::InvalidUriScheme);
        }

        // Connect and perform WebSocket handshake.
        let stream = TcpStream::connect(format!(
            "{}:{}",
            uri.host().unwrap_or_default(),
            uri.port_u16().unwrap_or(80)
        ))
        .await?;

        Ok(Self::new(handshake(stream, uri).await?))
    }
}

/// Performs a WebSocket handshake on an existing TCP connection via HTTP 1.
async fn handshake<T>(stream: T, uri: &Uri) -> ConnectResult<BufReader<T>>
where
    T: AsyncReadRent + AsyncWriteRent,
{
    let mut stream = BufReader::new(stream);

    // Generate a random key for the handshake.
    let mut rng = rand::rng();
    let mut key_bytes = [0u8; 16];
    rng.fill(&mut key_bytes);
    let key = BASE64_STANDARD.encode(key_bytes);

    // Create the HTTP request for the handshake.
    let request = http_request(uri, &key);

    // Send the handshake request.
    let (result, _) = stream.write_all(request.into_bytes()).await;
    result?;

    // Read the response.
    // let buffer = vec![0u8; 2048];
    // let (result, buffer) = stream.read(buffer).await;
    // let bytes_read = result.map_err(Error::Connect)?;
    // let response = String::from_utf8_lossy(&buffer[..bytes_read]);

    let mut response = String::with_capacity(2048);
    loop {
        let bytes_read = stream.read_line(&mut response).await?;
        // Ending is denoted with CRLF (2 bytes).
        if bytes_read <= 2 {
            break;
        }
    }

    // Verify the response status.
    if !response.starts_with("HTTP/1.1 101") {
        return Err(ConnectError::InvalidHandshakeResponse(response));
    }

    // Verify the server's accept key.
    let expected_accept = {
        let mut hasher = Sha1::new();
        hasher.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
        BASE64_STANDARD.encode(hasher.finalize())
    };
    if !response
        .to_lowercase()
        .contains(&format!("Sec-WebSocket-Accept: {expected_accept}").to_lowercase())
    {
        return Err(ConnectError::InvalidWebSocketAcceptHeader);
    }

    Ok(stream)
}

fn http_request(uri: &Uri, key: &str) -> String {
    let host = if let Some(port) = uri.port_u16() {
        format!("{}:{port}", uri.host().unwrap_or_default())
    } else {
        uri.host().unwrap_or_default().to_string()
    };

    format!(
        "GET {} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n",
        uri.path_and_query()
            .map(ToString::to_string)
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_request() {
        let output = http_request(
            &Uri::from_static("ws://localhost:9001/runCase?case=1&agent=monoio-ws"),
            "dGhlIHNhbXBsZSBub25jZQ==",
        );
        assert_eq!(
            output,
            "GET /runCase?case=1&agent=monoio-ws HTTP/1.1\r\n\
            Host: localhost:9001\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
            Sec-WebSocket-Version: 13\r\n\
            \r\n"
        )
    }
}
