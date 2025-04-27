use std::{io, mem, result, str, sync::LazyLock};

use monoio::io::{
    AsyncReadRent, AsyncReadRentExt, AsyncWriteRent, AsyncWriteRentExt, OwnedReadHalf,
    OwnedWriteHalf, Splitable,
};
use rand::{Rng, SeedableRng, rngs::SmallRng};

use crate::{CloseCode, Frame, Message, Opcode, io::AsyncReadRentExt as _};

pub static PROTOCOL_ERROR: LazyLock<Vec<u8>> = LazyLock::new(|| {
    u16::from(CloseCode::ProtocolError)
        .to_be_bytes()
        .into_iter()
        .collect()
});

pub struct Config {
    pub default_write_buffer_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_write_buffer_size: 4096,
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
                buffer_2: vec![0; 2],
                buffer_8: vec![0; 8],
            },
            write_half: WriteHalf {
                inner: write_half,
                rng: SmallRng::from_os_rng(),
                buffer: Vec::with_capacity(config.default_write_buffer_size),
            },
        }
    }
}

impl<S> Client<S>
where
    S: AsyncReadRent + AsyncWriteRent,
{
    pub async fn next_msg(&mut self, mut buffer: Vec<u8>) -> BufResult<Message> {
        buffer.clear();
        let mut message = None;
        let mut len = 0;

        loop {
            let is_empty = buffer.is_empty();
            let (res, buf) = self.read_frame(buffer).await;
            let frame = match res {
                Ok(frame) => frame,
                Err(e) => {
                    return (Err(e), buf);
                }
            };

            match frame.opcode {
                Opcode::Ping => {
                    // Auto-send pong frame.
                    if let Err(e) = self
                        .write_half
                        .write_control_frame(
                            Frame {
                                fin: true,
                                opcode: Opcode::Pong,
                            },
                            &buf[len..],
                        )
                        .await
                    {
                        return (Err(e.into()), buf);
                    }
                    match res {
                        Ok(_) => {
                            buffer = buf;
                            buffer.truncate(len);
                        }
                        Err(e) => {
                            return (Err(e), buf);
                        }
                    }
                }
                Opcode::Continuation => match message {
                    Some(Message::Text) => {
                        if frame.fin {
                            if Frame::validate_utf8(&buf).is_none() {
                                if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                    return (Err(e.into()), buf);
                                }
                                return (
                                    Err(Error::ProtocolViolation(
                                        "Received text frame with invalid utf-8.",
                                    )),
                                    buf,
                                );
                            }
                            return (Ok(Message::Text), buf);
                        }
                        buffer = buf;
                        len = buffer.len();
                    }
                    Some(Message::Binary) => {
                        if frame.fin {
                            return (Ok(Message::Binary), buf);
                        }
                        buffer = buf;
                        len = buffer.len();
                    }
                    None => {
                        if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                            return (Err(e.into()), buf);
                        }
                        return (
                            Err(Error::ProtocolViolation(
                                "Received continuation frame without preceding text or binary frame.",
                            )),
                            buf,
                        );
                    }
                },
                Opcode::Text => {
                    if frame.fin {
                        if !is_empty {
                            if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (
                                Err(Error::ProtocolViolation(
                                    "Received a continuation frame without continuation opcode.",
                                )),
                                buf,
                            );
                        }
                        if Frame::validate_utf8(&buf).is_none() {
                            if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (
                                Err(Error::ProtocolViolation(
                                    "Received text frame with invalid utf-8.",
                                )),
                                buf,
                            );
                        }
                        return (Ok(Message::Text), buf);
                    }
                    message = Some(Message::Text);
                    buffer = buf;
                    len = buffer.len();
                }
                Opcode::Binary => {
                    if frame.fin {
                        if !is_empty {
                            if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (
                                Err(Error::ProtocolViolation(
                                    "Received a continuation frame without continuation opcode.",
                                )),
                                buf,
                            );
                        }
                        return (Ok(Message::Binary), buf);
                    }
                    message = Some(Message::Binary);
                    buffer = buf;
                    len = buffer.len();
                }
                Opcode::Close => {
                    let frame_len = buf.len() - len;
                    let code = if frame_len >= 2 {
                        let Ok(close_code) =
                            CloseCode::try_from(u16::from_be_bytes([buf[len], buf[len + 1]]))
                        else {
                            if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (Err(Error::ProtocolViolation("Invalid close code.")), buf);
                        };
                        if close_code.is_reserved() {
                            if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (
                                Err(Error::ProtocolViolation("Received reserved close code.")),
                                buf,
                            );
                        }
                        Some(close_code)
                    } else {
                        None
                    };
                    // Everything after close code is a utf-8 reason string.
                    let reason = if frame_len > 2 {
                        let Some(reason) = Frame::validate_utf8(&buf[len + 2..]) else {
                            if let Err(e) = self.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (
                                Err(Error::ProtocolViolation(
                                    "Received close frame with invalid utf-8 reason.",
                                )),
                                buf,
                            );
                        };
                        Some(reason.to_owned())
                    } else {
                        None
                    };

                    // Auto-send close with the same code as received.
                    if let Err(e) = self
                        .write_half
                        .write_control_frame(
                            Frame {
                                fin: true,
                                opcode: Opcode::Close,
                            },
                            &buf[len..],
                        )
                        .await
                    {
                        return (Err(e.into()), buf);
                    }
                    return match res {
                        Ok(_) => (Err(Error::Closed { code, reason }), buf),
                        Err(e) => (Err(e), buf),
                    };
                }
                Opcode::Pong => {
                    buffer = buf;
                    buffer.truncate(len);
                }
                _ => unreachable!(),
            }
        }
    }

    pub async fn read_frame(&mut self, buffer: Vec<u8>) -> BufResult<Frame> {
        match self.read_half.read_frame(buffer).await {
            (Ok(res), buffer) => (Ok(res), buffer),
            (Err(Error::ProtocolViolation(reason)), buffer) => {
                match self.send_close(&PROTOCOL_ERROR).await {
                    Ok(()) => (Err(Error::ProtocolViolation(reason)), buffer),
                    Err(err) => (Err(err.into()), buffer),
                }
            }
            (Err(err), buffer) => (Err(err), buffer),
        }
    }

    pub async fn send_ping(&mut self, buf: &[u8]) -> io::Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Ping,
            },
            buf,
        )
        .await
    }

    pub async fn send_pong(&mut self, buf: &[u8]) -> io::Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Pong,
            },
            buf,
        )
        .await
    }

    pub async fn send_binary(&mut self, buf: &[u8]) -> io::Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Binary,
            },
            buf,
        )
        .await
    }

    pub async fn send_text(&mut self, buf: &[u8]) -> io::Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Text,
            },
            buf,
        )
        .await
    }

    pub async fn send_close(&mut self, buf: &[u8]) -> io::Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Close,
            },
            buf,
        )
        .await
    }

    #[inline]
    async fn send(&mut self, frame: Frame, buf: &[u8]) -> io::Result<()> {
        self.write_frame(frame, buf).await
    }

    pub async fn write_frame(&mut self, frame: Frame, buffer: &[u8]) -> io::Result<()> {
        self.write_half.write_frame(frame, buffer).await
    }
}

struct ReadHalf<S> {
    inner: OwnedReadHalf<S>,
    buffer_2: Vec<u8>,
    buffer_8: Vec<u8>,
}

impl<S> ReadHalf<S>
where
    S: AsyncReadRent,
{
    pub async fn read_frame(&mut self, buffer: Vec<u8>) -> BufResult<Frame> {
        let buffer_2 = mem::take(&mut self.buffer_2);
        let (res, buffer_2) = self.inner.read_exact(buffer_2).await;
        self.buffer_2 = buffer_2;
        match res {
            Ok(_) => {}
            Err(e) => {
                return (Err(e.into()), buffer);
            }
        }
        let b1 = self.buffer_2[0];
        let b2 = self.buffer_2[1];

        let fin = b1 & 0x80 != 0;
        let rsv = b1 & 0x70;
        let opcode = unsafe { mem::transmute::<u8, Opcode>(b1 & 0x0F) };
        let masked = b2 & 0x80 != 0;
        let mut length = (b2 & 0x7F) as usize;

        if rsv != 0 {
            return (
                Err(Error::ProtocolViolation("Reserve bit must be 0.")),
                buffer,
            );
        }
        if masked {
            return (
                Err(Error::ProtocolViolation(
                    "Server to client communication should be unmasked.",
                )),
                buffer,
            );
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
                return (
                    Err(Error::ProtocolViolation("Use of reserved opcode.")),
                    buffer,
                );
            }
            Opcode::Close => {
                if length == 1 {
                    return (
                        Err(Error::ProtocolViolation(
                            "Close frame with a missing close reason byte.",
                        )),
                        buffer,
                    );
                }
                if length > 125 {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame larger than 125 bytes.",
                        )),
                        buffer,
                    );
                }
                if !fin {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame cannot be fragmented.",
                        )),
                        buffer,
                    );
                }
            }
            Opcode::Ping | Opcode::Pong => {
                if length > 125 {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame larger than 125 bytes.",
                        )),
                        buffer,
                    );
                }
                if !fin {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame cannot be fragmented.",
                        )),
                        buffer,
                    );
                }
            }
            Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                length = match length {
                    126 => {
                        const LENGTH_LEN: usize = 2;
                        let buffer_2 = mem::take(&mut self.buffer_2);
                        let (res, buffer_2) = self.inner.read_exact(buffer_2).await;
                        self.buffer_2 = buffer_2;
                        match res {
                            Ok(_) => {}
                            Err(e) => {
                                return (Err(e.into()), buffer);
                            }
                        }
                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(&self.buffer_2);
                        u16::from_be_bytes(bytes) as usize
                    }
                    127 => {
                        const LENGTH_LEN: usize = 8;
                        let buffer_8 = mem::take(&mut self.buffer_8);
                        let (res, buffer_8) = self.inner.read_exact(buffer_8).await;
                        self.buffer_8 = buffer_8;
                        match res {
                            Ok(_) => {}
                            Err(e) => {
                                return (Err(e.into()), buffer);
                            }
                        }
                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(&self.buffer_8);
                        u64::from_be_bytes(bytes) as usize
                    }
                    length => length,
                };
            }
        }

        let (res, buffer) = self.inner.read_extend(buffer, length).await;
        match res {
            Ok(_) => {}
            Err(e) => {
                return (Err(e.into()), buffer);
            }
        }

        let frame = Frame { fin, opcode };
        (Ok(frame), buffer)
    }
}

struct WriteHalf<S>
where
    S: AsyncWriteRent,
{
    inner: OwnedWriteHalf<S>,
    rng: SmallRng,
    buffer: Vec<u8>,
}

impl<S> WriteHalf<S>
where
    S: AsyncWriteRent,
{
    pub async fn write_frame(&mut self, frame: Frame, data: &[u8]) -> io::Result<()> {
        let mut dst = mem::take(&mut self.buffer);
        frame.encode(data, &mut dst, self.rng.random::<u32>().to_ne_bytes());
        let (res, buffer) = self.inner.write_all(dst).await;
        self.buffer = buffer;
        res.map(|_| ())
    }

    pub async fn write_control_frame(&mut self, frame: Frame, data: &[u8]) -> io::Result<()> {
        let mut dst = mem::take(&mut self.buffer);
        frame.encode_control(data, &mut dst, self.rng.random::<u32>().to_ne_bytes());
        let (res, buffer) = self.inner.write_all(dst).await;
        self.buffer = buffer;
        res.map(|_| ())
    }
}
