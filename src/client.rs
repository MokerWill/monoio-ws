use std::{io, mem, result, str, sync::LazyLock};

use bytes::{BytesMut, Buf};
use monoio::io::{
    AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt, OwnedReadHalf, OwnedWriteHalf, Splitable,
};
use rand::{Rng, SeedableRng, rngs::SmallRng};

use crate::{CloseCode, Frame, Message, Opcode};

pub static PROTOCOL_ERROR: LazyLock<Vec<u8>> = LazyLock::new(|| {
    u16::from(CloseCode::ProtocolError)
        .to_be_bytes()
        .into_iter()
        .collect()
});

pub struct Config {
    pub read_buffer_capacity: usize,
    pub write_buffer_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            read_buffer_capacity: 128 * 1024,
            write_buffer_capacity: 128 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO: {0}")]
    Io(#[from] io::Error),
    #[error("Protocol violation: {0}")]
    ProtocolViolation(&'static str),
    #[error("The connection has been closed: {code:?} {reason:?}.")]
    Closed {
        code: Option<CloseCode>,
        reason: Option<String>,
    },
}

pub type BufResult<T> = (result::Result<T, Error>, Vec<u8>);
pub type Result<T> = result::Result<T, Error>;

pub struct Client<S>
where
    S: AsyncWriteRent,
{
    read_half: ReadHalf<S>,
    write_half: WriteHalf<S>,
}

impl<S> Client<S>
where
    S: AsyncWriteRent + Splitable<OwnedRead = OwnedReadHalf<S>, OwnedWrite = OwnedWriteHalf<S>>,
{
    pub fn new(stream: S, config: &Config) -> Self {
        let (read_half, write_half) = stream.into_split();
        Self {
            read_half: ReadHalf {
                inner: read_half,
                buffer: BytesMut::with_capacity(config.read_buffer_capacity),
            },
            write_half: WriteHalf {
                inner: write_half,
                rng: SmallRng::from_os_rng(),
                buffer: BytesMut::with_capacity(config.write_buffer_capacity),
            },
        }
    }
}

impl<S> Client<S>
where
    S: AsyncReadRent + AsyncWriteRent,
{
    pub async fn next_msg(&mut self, buffer: Vec<u8>) -> BufResult<Message> {
        self.read_half.next_msg(&mut self.write_half, buffer).await
    }

    pub async fn read_frame(&mut self) -> Result<Frame<'_>> {
        self.read_half.read_frame(&mut self.write_half).await
    }

    pub async fn send_ping(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_half.send_ping(data).await
    }

    pub async fn send_pong(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_half.send_pong(data).await
    }

    pub async fn send_binary(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_half.send_binary(data).await
    }

    pub async fn send_text(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_half.send_text(data).await
    }

    pub async fn send_close(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_half.send_text(data).await
    }

    pub async fn write_frame(&mut self, frame: Frame<'_>) -> io::Result<()> {
        self.write_half.write_frame(frame).await
    }
}

struct ReadHalf<S> {
    inner: OwnedReadHalf<S>,
    buffer: BytesMut,
}

impl<S> ReadHalf<S>
where
    S: AsyncReadRent + AsyncWriteRent,
{
    const CHUNK_SIZE: usize = 4096;

    pub async fn next_msg<'a>(
        &'a mut self,
        write: &'a mut WriteHalf<S>,
        mut buffer: Vec<u8>,
    ) -> BufResult<Message> {
        buffer.clear();
        let mut message = None;

        loop {
            let is_empty = buffer.is_empty();
            let frame = match self.read_frame(write).await {
                Ok(frame) => frame,
                Err(e) => return (Err(e), buffer),
            };

            match frame.opcode {
                Opcode::Continuation => match message {
                    Some(Message::Text) if frame.fin => {
                        buffer.extend_from_slice(frame.data);
                        if Frame::validate_utf8(&buffer).is_some() {
                            return (Ok(Message::Text), buffer);
                        } else {
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buffer);
                            };
                            return (
                                Err(Error::ProtocolViolation(
                                    "Received text frame with invalid utf-8.",
                                )),
                                buffer,
                            );
                        }
                    }
                    Some(Message::Binary) if frame.fin => {
                        buffer.extend_from_slice(frame.data);
                        return (Ok(Message::Binary), buffer);
                    }
                    Some(Message::Text | Message::Binary) => {
                        buffer.extend_from_slice(frame.data);
                    }
                    None => {
                        if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
                            return (Err(e.into()), buffer);
                        };
                        return (
                            Err(Error::ProtocolViolation(
                                "Received continuation frame without preceding text or binary frame.",
                            )),
                            buffer,
                        );
                    }
                },
                Opcode::Text if frame.fin => {
                    if !is_empty {
                        if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
                            return (Err(e.into()), buffer);
                        };
                        return (
                            Err(Error::ProtocolViolation(
                                "Received a continuation frame without continuation opcode.",
                            )),
                            buffer,
                        );
                    }
                    if Frame::validate_utf8(frame.data).is_some() {
                        buffer.extend_from_slice(frame.data);
                        return (Ok(Message::Text), buffer);
                    } else {
                        if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
                            return (Err(e.into()), buffer);
                        };
                        return (
                            Err(Error::ProtocolViolation(
                                "Received text frame with invalid utf-8.",
                            )),
                            buffer,
                        );
                    }
                }
                Opcode::Text => {
                    buffer.extend_from_slice(frame.data);
                    message = Some(Message::Text);
                }
                Opcode::Binary if frame.fin => {
                    if !is_empty {
                        if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
                            return (Err(e.into()), buffer);
                        };
                        return (
                            Err(Error::ProtocolViolation(
                                "Received a continuation frame without continuation opcode.",
                            )),
                            buffer,
                        );
                    }
                    buffer.extend_from_slice(frame.data);
                    return (Ok(Message::Binary), buffer);
                }
                Opcode::Binary => {
                    buffer.extend_from_slice(frame.data);
                    message = Some(Message::Binary);
                }
                Opcode::Close => {
                    let code = if frame.data.len() >= 2 {
                        // Ok to unwrap as code is already validated.
                        let close_code =
                            CloseCode::try_from(u16::from_be_bytes([frame.data[0], frame.data[1]]))
                                .unwrap();
                        Some(close_code)
                    } else {
                        None
                    };
                    // Everything after close code is a utf-8 reason string.
                    let reason = if frame.data.len() > 2 {
                        Some(unsafe { str::from_utf8_unchecked(&frame.data[2..]) })
                    } else {
                        None
                    };
                    return (
                        Err(Error::Closed {
                            code,
                            reason: reason.map(ToOwned::to_owned),
                        }),
                        buffer,
                    );
                }
                Opcode::Ping | Opcode::Pong => {}
                _ => unreachable!(),
            }
        }
    }

    pub async fn read_frame<'a>(&'a mut self, write: &mut WriteHalf<S>) -> Result<Frame<'a>> {
        match self.read_frame_inner().await {
            Ok(frame) if matches!(frame.opcode, Opcode::Ping) => {
                // Auto-send pong frame.
                write
                    .write_control_frame(Frame {
                        fin: true,
                        opcode: Opcode::Pong,
                        data: frame.data,
                    })
                    .await?;
                Ok(frame)
            }
            Ok(frame) if matches!(frame.opcode, Opcode::Close) => {
                let _code = if frame.data.len() >= 2 {
                    let Ok(close_code) =
                        CloseCode::try_from(u16::from_be_bytes([frame.data[0], frame.data[1]]))
                    else {
                        write.send_close(&PROTOCOL_ERROR).await?;
                        return Err(Error::ProtocolViolation("Invalid close code."));
                    };
                    if close_code.is_reserved() {
                        write.send_close(&PROTOCOL_ERROR).await?;
                        return Err(Error::ProtocolViolation("Received reserved close code."));
                    }
                    Some(close_code)
                } else {
                    None
                };
                // Everything after close code is a utf-8 reason string.
                let _reason = if frame.data.len() > 2 {
                    let Some(reason) = Frame::validate_utf8(&frame.data[2..]) else {
                        write.send_close(&PROTOCOL_ERROR).await?;
                        return Err(Error::ProtocolViolation(
                            "Received close frame with invalid utf-8 reason.",
                        ));
                    };
                    Some(reason)
                } else {
                    None
                };
                // Auto-send close with the same code as received.
                write
                    .write_control_frame(Frame {
                        fin: true,
                        opcode: Opcode::Close,
                        data: frame.data,
                    })
                    .await?;
                Ok(frame)
            }
            Ok(frame) => Ok(frame),
            Err(Error::ProtocolViolation(reason)) => {
                write.send_close(&PROTOCOL_ERROR).await?;
                Err(Error::ProtocolViolation(reason))
            }
            Err(err) => Err(err),
        }
    }

    #[inline]
    async fn read_frame_inner(&mut self) -> Result<Frame<'static>> {
        const HEADER_LEN: usize = 2;

        // Compact buffer if we've consumed more than half of it
        if self.buffer.len() > self.buffer.capacity() / 2 {
            self.buffer.advance(self.buffer.len());
        }

        self.ensure_read(HEADER_LEN).await?;

        // Read header bytes directly from buffer
        let b1 = self.buffer[0];
        let b2 = self.buffer[1];

        let fin = b1 & 0x80 != 0;
        let rsv = b1 & 0x70;
        let opcode = unsafe { mem::transmute::<u8, Opcode>(b1 & 0x0F) };
        let masked = b2 & 0x80 != 0;
        let mut length = (b2 & 0x7F) as usize;

        if rsv != 0 {
            return Err(Error::ProtocolViolation("Reserve bit must be 0."));
        }
        if masked {
            return Err(Error::ProtocolViolation(
                "Server to client communication should be unmasked.",
            ));
        }

        match opcode {
            Opcode::Reserved3
            | Opcode::Reserved4
            | Opcode::Reserved5
            | Opcode::Reserved6
            | Opcode::Reserved7
            | Opcode::ReservedB
            | Opcode::ReservedC
            | Opcode::ReservedD
            | Opcode::ReservedE
            | Opcode::ReservedF => {
                return Err(Error::ProtocolViolation("Use of reserved opcode."));
            }
            Opcode::Close => {
                if length == 1 {
                    return Err(Error::ProtocolViolation(
                        "Close frame with a missing close reason byte.",
                    ));
                }
                if length > 125 {
                    return Err(Error::ProtocolViolation(
                        "Control frame larger than 125 bytes.",
                    ));
                }
                if !fin {
                    return Err(Error::ProtocolViolation(
                        "Control frame cannot be fragmented.",
                    ));
                }
            }
            Opcode::Ping | Opcode::Pong => {
                if length > 125 {
                    return Err(Error::ProtocolViolation(
                        "Control frame larger than 125 bytes.",
                    ));
                }
                if !fin {
                    return Err(Error::ProtocolViolation(
                        "Control frame cannot be fragmented.",
                    ));
                }
            }
            Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                length = match length {
                    126 => {
                        const LENGTH_LEN: usize = 2;

                        self.ensure_read(HEADER_LEN + LENGTH_LEN).await?;
                        let bytes = [self.buffer[2], self.buffer[3]];
                        u16::from_be_bytes(bytes) as usize
                    }
                    127 => {
                        const LENGTH_LEN: usize = 8;

                        self.ensure_read(HEADER_LEN + LENGTH_LEN).await?;
                        let bytes = [
                            self.buffer[2], self.buffer[3], self.buffer[4], self.buffer[5],
                            self.buffer[6], self.buffer[7], self.buffer[8], self.buffer[9],
                        ];
                        u64::from_be_bytes(bytes) as usize
                    }
                    length => length,
                };
            }
        }

        self.ensure_read(HEADER_LEN + length).await?;

        // Extract data using split_to and convert to static reference
        self.buffer.advance(HEADER_LEN);
        let data_bytes = self.buffer.split_to(length).freeze();
        let data_static: &'static [u8] = Box::leak(data_bytes.to_vec().into_boxed_slice());
        
        Ok(Frame { fin, opcode, data: data_static })
    }

    #[inline]
    async fn ensure_read(&mut self, len: usize) -> Result<()> {
        while self.buffer.len() < len {
            let remaining = len - self.buffer.len();
            let to_read = remaining.max(Self::CHUNK_SIZE);
            
            let (res, buffer) = self.inner.read(BytesMut::with_capacity(to_read)).await;
            let n = res?;
            
            if n > 0 {
                self.buffer.extend_from_slice(&buffer[..n]);
            } else {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed while reading",
                )));
            }
        }
        Ok(())
    }
}

struct WriteHalf<S>
where
    S: AsyncWriteRent,
{
    inner: OwnedWriteHalf<S>,
    rng: SmallRng,
    buffer: BytesMut,
}

impl<S> WriteHalf<S>
where
    S: AsyncWriteRent,
{
    pub async fn send_ping(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Ping,
            data,
        })
        .await
    }

    pub async fn send_pong(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Pong,
            data,
        })
        .await
    }

    pub async fn send_binary(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Binary,
            data,
        })
        .await
    }

    pub async fn send_text(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Text,
            data,
        })
        .await
    }

    pub async fn send_close(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(Frame {
            fin: true,
            opcode: Opcode::Close,
            data,
        })
        .await
    }

    #[inline]
    async fn send(&mut self, frame: Frame<'_>) -> io::Result<()> {
        self.write_frame(frame).await
    }

    pub async fn write_frame(&mut self, frame: Frame<'_>) -> io::Result<()> {
        let mut dst = mem::take(&mut self.buffer).freeze().to_vec();
        frame.encode(&mut dst, self.rng.random::<u32>().to_ne_bytes());
        let (res, buffer) = self.inner.write_all(dst).await;
        self.buffer = BytesMut::from_iter(buffer);
        res.map(|_| ())
    }

    pub async fn write_control_frame(&mut self, frame: Frame<'_>) -> io::Result<()> {
        let mut dst = mem::take(&mut self.buffer).freeze().to_vec();
        frame.encode_control(&mut dst, self.rng.random::<u32>().to_ne_bytes());
        let (res, buffer) = self.inner.write_all(dst).await;
        self.buffer = BytesMut::from_iter(buffer);
        res.map(|_| ())
    }
}
