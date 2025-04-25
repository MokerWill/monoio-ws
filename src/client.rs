use std::{io, mem, str, sync::LazyLock};

use monoio::io::{
    AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt, OwnedReadHalf, OwnedWriteHalf, Splitable,
};
use rand::{Rng, SeedableRng, rngs::SmallRng};

use crate::{
    CloseCode, Frame, Message, Opcode,
    io::{AsyncReadRentExt as _, AsyncWriteRentExt as _},
};

macro_rules! protocol_violation {
    ($self:expr, $reason:expr) => {
        let reason: &'static str = $reason;
        let (res, buffer) = $self.send_close(PROTOCOL_ERROR.clone()).await;
        if let Err(e) = res {
            return (Err(e.into()), buffer);
        }
        return (Err(Error::ProtocolViolation(reason)), buffer);
    };
}

pub static PROTOCOL_ERROR: LazyLock<Vec<u8>> = LazyLock::new(|| {
    u16::from(CloseCode::ProtocolError)
        .to_be_bytes()
        .into_iter()
        .collect()
});

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

pub type Result<T> = (std::result::Result<T, Error>, Vec<u8>);

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
    pub fn new(stream: S) -> Self {
        let (read_half, write_half) = stream.into_split();
        Self {
            read_half: ReadHalf { inner: read_half },
            write_half: WriteHalf {
                inner: write_half,
                rng: SmallRng::from_os_rng(),
            },
        }
    }
}

impl<S> Client<S>
where
    S: AsyncReadRent + AsyncWriteRent,
{
    pub async fn next_msg(&mut self, mut buffer: Vec<u8>) -> Result<Message> {
        buffer.clear();
        let mut message = None;
        let mut len = 0;

        loop {
            let is_empty = buffer.is_empty();
            let (res, mut buf) = self.read_frame(buffer).await;
            let frame = match res {
                Ok(frame) => frame,
                Err(e) => {
                    return (Err(e), buf);
                }
            };

            match frame.opcode {
                Opcode::Ping => {
                    // Write frame.
                    buf.resize(buf.len() + Frame::CONTROL_HEADER_LEN, 0);
                    let pong_frame = Frame {
                        fin: true,
                        opcode: Opcode::Pong,
                    };
                    let (res, buf) = self
                        .write_half
                        .write_control_frame_offset(pong_frame, buf, len)
                        .await;
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
                                protocol_violation!(
                                    self,
                                    "Received text frame with invalid utf-8."
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
                        protocol_violation!(
                            self,
                            "Received continuation frame without preceding text or binary frame."
                        );
                    }
                },
                Opcode::Text => {
                    if frame.fin {
                        if !is_empty {
                            protocol_violation!(
                                self,
                                "Received a continuation frame without continuation opcode."
                            );
                        }
                        if Frame::validate_utf8(&buf).is_none() {
                            protocol_violation!(self, "Received text frame with invalid utf-8.");
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
                            protocol_violation!(
                                self,
                                "Received a continuation frame without continuation opcode."
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
                            protocol_violation!(self, "Invalid close code.");
                        };
                        if close_code.is_reserved() {
                            protocol_violation!(self, "Received reserved close code.");
                        }
                        Some(close_code)
                    } else {
                        None
                    };
                    // Everything after close code is a utf-8 reason string.
                    let reason = if frame_len > 2 {
                        let Some(reason) = Frame::validate_utf8(&buf[len + 2..]) else {
                            protocol_violation!(
                                self,
                                "Received close frame with invalid utf-8 reason."
                            );
                        };
                        Some(reason.to_owned())
                    } else {
                        None
                    };

                    // Reply to close with the same code.
                    buf.resize(buf.len() + Frame::CONTROL_HEADER_LEN, 0);
                    let close_frame = Frame {
                        fin: true,
                        opcode: Opcode::Close,
                    };
                    let (res, buf) = self
                        .write_half
                        .write_control_frame_offset(close_frame, buf, len)
                        .await;
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

    pub async fn read_frame(&mut self, buffer: Vec<u8>) -> Result<Frame> {
        match self.read_half.read_frame(buffer).await {
            (Ok(res), buffer) => (Ok(res), buffer),
            (Err(Error::ProtocolViolation(reason)), buffer) => {
                match self.send_close(PROTOCOL_ERROR.clone()).await {
                    (Ok(()), _) => (Err(Error::ProtocolViolation(reason)), buffer),
                    (Err(err), _) => (Err(err), buffer),
                }
            }
            (Err(err), buffer) => (Err(err), buffer),
        }
    }

    pub async fn send_ping(&mut self, buf: Vec<u8>) -> Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Ping,
            },
            buf,
        )
        .await
    }

    pub async fn send_pong(&mut self, buf: Vec<u8>) -> Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Pong,
            },
            buf,
        )
        .await
    }

    pub async fn send_binary(&mut self, buf: Vec<u8>) -> Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Binary,
            },
            buf,
        )
        .await
    }

    pub async fn send_text(&mut self, buf: Vec<u8>) -> Result<()> {
        self.send(
            Frame {
                fin: true,
                opcode: Opcode::Text,
            },
            buf,
        )
        .await
    }

    pub async fn send_close(&mut self, buf: Vec<u8>) -> Result<()> {
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
    async fn send(&mut self, frame: Frame, buf: Vec<u8>) -> Result<()> {
        let (res, mut buf) = self.write_frame(frame, buf).await;
        buf.clear();
        (res, buf)
    }

    pub async fn write_frame(&mut self, frame: Frame, buffer: Vec<u8>) -> Result<()> {
        self.write_half.write_frame(frame, buffer).await
    }
}

struct ReadHalf<S> {
    inner: OwnedReadHalf<S>,
}

impl<S> ReadHalf<S>
where
    S: AsyncReadRent,
{
    pub async fn read_frame(&mut self, mut buffer: Vec<u8>) -> Result<Frame> {
        const HEADER_LEN: usize = 2;
        let data_len = buffer.len();

        buffer.resize(data_len + HEADER_LEN, 0);
        let (res, mut buffer) = self.inner.read_extend(buffer, HEADER_LEN).await;
        match res {
            Ok(_) => {}
            Err(e) => {
                return (Err(e.into()), buffer);
            }
        }
        let b1 = buffer[buffer.len() - 2];
        let b2 = buffer[buffer.len() - 1];
        buffer.truncate(data_len);

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
                (length, buffer) = match length {
                    126 => {
                        const LENGTH_LEN: usize = 2;
                        buffer.resize(data_len + LENGTH_LEN, 0);
                        let (res, mut buffer) = self.inner.read_extend(buffer, LENGTH_LEN).await;
                        match res {
                            Ok(_) => {}
                            Err(e) => {
                                return (Err(e.into()), buffer);
                            }
                        }
                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(&buffer[buffer.len() - LENGTH_LEN..]);
                        buffer.truncate(data_len);
                        (u16::from_be_bytes(bytes) as usize, buffer)
                    }
                    127 => {
                        const LENGTH_LEN: usize = 8;
                        buffer.resize(data_len + LENGTH_LEN, 0);
                        let (res, mut buffer) = self.inner.read_extend(buffer, LENGTH_LEN).await;
                        match res {
                            Ok(_) => {}
                            Err(e) => {
                                return (Err(e.into()), buffer);
                            }
                        }
                        let mut bytes = [0u8; LENGTH_LEN];
                        bytes.copy_from_slice(&buffer[buffer.len() - LENGTH_LEN..]);
                        buffer.truncate(data_len);
                        (u64::from_be_bytes(bytes) as usize, buffer)
                    }
                    length => (length, buffer),
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
}

impl<S> WriteHalf<S>
where
    S: AsyncWriteRent,
{
    pub async fn write_frame(&mut self, frame: Frame, mut buffer: Vec<u8>) -> Result<()> {
        frame.encode_vec(&mut buffer, self.rng.random::<u32>().to_ne_bytes());
        let (res, buffer) = self.inner.write_all(buffer).await;
        match res {
            Ok(_) => {}
            Err(e) => return (Err(e.into()), buffer),
        }
        (Ok(()), buffer)
    }

    pub async fn write_control_frame_offset(
        &mut self,
        frame: Frame,
        mut buffer: Vec<u8>,
        offset: usize,
    ) -> Result<()> {
        frame.encode_control_slice(
            &mut buffer[offset..],
            self.rng.random::<u32>().to_ne_bytes(),
        );
        let (res, buffer) = self.inner.write_offset(buffer, offset).await;
        match res {
            Ok(_) => {}
            Err(e) => return (Err(e.into()), buffer),
        }
        (Ok(()), buffer)
    }
}
