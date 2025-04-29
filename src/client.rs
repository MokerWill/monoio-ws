use std::{io, mem, result, str, sync::LazyLock};

use monoio::io::{
    AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt, OwnedReadHalf, OwnedWriteHalf, Splitable,
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
                buffer: Vec::with_capacity(config.read_buffer_capacity),
                consumed: 0,
            },
            write_half: WriteHalf {
                inner: write_half,
                rng: SmallRng::from_os_rng(),
                buffer: Vec::with_capacity(config.write_buffer_capacity),
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

    pub async fn read_frame(&mut self, buffer: Vec<u8>) -> BufResult<Frame> {
        self.read_half
            .read_frame(&mut self.write_half, buffer)
            .await
    }

    pub async fn send_ping(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_half.send_ping(buf).await
    }

    pub async fn send_pong(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_half.send_pong(buf).await
    }

    pub async fn send_binary(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_half.send_binary(buf).await
    }

    pub async fn send_text(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_half.send_text(buf).await
    }

    pub async fn send_close(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_half.send_text(buf).await
    }

    pub async fn write_frame(&mut self, frame: Frame, buffer: &[u8]) -> io::Result<()> {
        self.write_half.write_frame(frame, buffer).await
    }
}

struct ReadHalf<S> {
    inner: OwnedReadHalf<S>,
    buffer: Vec<u8>,
    consumed: usize,
}

impl<S> ReadHalf<S>
where
    S: AsyncReadRent + AsyncWriteRent,
{
    const CHUNK_SIZE: usize = 4096;

    pub async fn next_msg(
        &mut self,
        write: &mut WriteHalf<S>,
        mut buffer: Vec<u8>,
    ) -> BufResult<Message> {
        buffer.clear();
        let mut message = None;
        let mut len = 0;

        loop {
            let is_empty = buffer.is_empty();
            let (res, buf) = self.read_frame(write, buffer).await;
            let frame = match res {
                Ok(frame) => frame,
                Err(e) => {
                    return (Err(e), buf);
                }
            };

            match frame.opcode {
                Opcode::Ping => {
                    // Auto-send pong frame.
                    if let Err(e) = write
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
                                if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                        if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
                                return (Err(e.into()), buf);
                            }
                            return (Err(Error::ProtocolViolation("Invalid close code.")), buf);
                        };
                        if close_code.is_reserved() {
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                            if let Err(e) = write.send_close(&PROTOCOL_ERROR).await {
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
                    if let Err(e) = write
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

    pub async fn read_frame(
        &mut self,
        write: &mut WriteHalf<S>,
        buffer: Vec<u8>,
    ) -> BufResult<Frame> {
        match self.read_frame_inner(buffer).await {
            (Ok(res), buffer) => (Ok(res), buffer),
            (Err(Error::ProtocolViolation(reason)), buffer) => {
                match write.send_close(&PROTOCOL_ERROR).await {
                    Ok(()) => (Err(Error::ProtocolViolation(reason)), buffer),
                    Err(err) => (Err(err.into()), buffer),
                }
            }
            (Err(err), buffer) => (Err(err), buffer),
        }
    }

    async fn read_frame_inner(&mut self, mut output: Vec<u8>) -> BufResult<Frame> {
        const HEADER_LEN: usize = 2;

        if self.consumed > 0 && self.buffer.len() > self.buffer.capacity() - Self::CHUNK_SIZE {
            self.buffer.drain(..self.consumed);
            self.consumed = 0;
        }

        while self.buffer.len() < self.consumed + HEADER_LEN {
            let buffer = mem::take(&mut self.buffer);
            let (res, buffer) = self.inner.read_extend(buffer, Self::CHUNK_SIZE).await;
            self.buffer = buffer;
            if let Err(e) = res {
                return (Err(e.into()), output);
            }
        }

        let b1 = self.buffer[self.consumed];
        let b2 = self.buffer[self.consumed + 1];
        self.consumed += HEADER_LEN;

        let fin = b1 & 0x80 != 0;
        let rsv = b1 & 0x70;
        let opcode = unsafe { mem::transmute::<u8, Opcode>(b1 & 0x0F) };
        let masked = b2 & 0x80 != 0;
        let mut length = (b2 & 0x7F) as usize;

        if rsv != 0 {
            return (
                Err(Error::ProtocolViolation("Reserve bit must be 0.")),
                output,
            );
        }
        if masked {
            return (
                Err(Error::ProtocolViolation(
                    "Server to client communication should be unmasked.",
                )),
                output,
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
                    output,
                );
            }
            Opcode::Close => {
                if length == 1 {
                    return (
                        Err(Error::ProtocolViolation(
                            "Close frame with a missing close reason byte.",
                        )),
                        output,
                    );
                }
                if length > 125 {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame larger than 125 bytes.",
                        )),
                        output,
                    );
                }
                if !fin {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame cannot be fragmented.",
                        )),
                        output,
                    );
                }
            }
            Opcode::Ping | Opcode::Pong => {
                if length > 125 {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame larger than 125 bytes.",
                        )),
                        output,
                    );
                }
                if !fin {
                    return (
                        Err(Error::ProtocolViolation(
                            "Control frame cannot be fragmented.",
                        )),
                        output,
                    );
                }
            }
            Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                length = match length {
                    126 => {
                        const LENGTH_LEN: usize = 2;

                        while self.buffer.len() < self.consumed + LENGTH_LEN {
                            let buffer = mem::take(&mut self.buffer);
                            let (res, buffer) =
                                self.inner.read_extend(buffer, Self::CHUNK_SIZE).await;
                            self.buffer = buffer;
                            if let Err(e) = res {
                                return (Err(e.into()), output);
                            }
                        }

                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(
                            &self.buffer[self.consumed..self.consumed + LENGTH_LEN],
                        );
                        self.consumed += LENGTH_LEN;
                        u16::from_be_bytes(bytes) as usize
                    }
                    127 => {
                        const LENGTH_LEN: usize = 8;

                        while self.buffer.len() < self.consumed + LENGTH_LEN {
                            let buffer = mem::take(&mut self.buffer);
                            let (res, buffer) =
                                self.inner.read_extend(buffer, Self::CHUNK_SIZE).await;
                            self.buffer = buffer;
                            if let Err(e) = res {
                                return (Err(e.into()), output);
                            }
                        }

                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(
                            &self.buffer[self.consumed..self.consumed + LENGTH_LEN],
                        );
                        self.consumed += LENGTH_LEN;
                        u64::from_be_bytes(bytes) as usize
                    }
                    length => length,
                };
            }
        }

        while self.buffer.len() < self.consumed + length {
            let buffer = mem::take(&mut self.buffer);
            let (res, buffer) = self.inner.read_extend(buffer, Self::CHUNK_SIZE).await;
            self.buffer = buffer;
            if let Err(e) = res {
                return (Err(e.into()), output);
            }
        }

        output.extend_from_slice(&self.buffer[self.consumed..self.consumed + length]);
        self.consumed += length;

        let frame = Frame { fin, opcode };
        (Ok(frame), output)
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
