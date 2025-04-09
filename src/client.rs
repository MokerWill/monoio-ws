use std::{str, sync::LazyLock};

use monoio::{
    BufResult,
    buf::{IoBufMut, Slice, SliceMut},
    io::{AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt},
};
use rand::{Rng, SeedableRng, rngs::SmallRng};

use crate::{CloseCode, Frame, Message, Opcode};

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
    Io(#[from] std::io::Error),
    #[error("Protocol violation: {0}")]
    ProtocolViolation(&'static str),
    #[error("The connection has been closed: {code:?} {reason:?}.")]
    Closed {
        code: Option<CloseCode>,
        reason: Option<String>,
    },
}

pub type Result<T> = (std::result::Result<T, Error>, Vec<u8>);

pub struct Client<S> {
    stream: S,
    rng: SmallRng,
}

impl<S> Client<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            rng: SmallRng::from_os_rng(),
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
                    pong_frame.encode_control_slice(
                        &mut buf[len..],
                        self.rng.random::<u32>().to_ne_bytes(),
                    );
                    let (res, buf) = self.write_offset(buf, len).await;
                    match res {
                        Ok(_) => {
                            buffer = buf;
                            buffer.truncate(len);
                        }
                        Err(e) => {
                            return (Err(e.into()), buf);
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
                    close_frame.encode_control_slice(
                        &mut buf[len..],
                        self.rng.random::<u32>().to_ne_bytes(),
                    );
                    let (res, buf) = self.write_offset(buf, len).await;
                    return match res {
                        Ok(_) => (Err(Error::Closed { code, reason }), buf),
                        Err(e) => (Err(e.into()), buf),
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

    pub async fn read_frame(&mut self, mut buffer: Vec<u8>) -> Result<Frame> {
        const HEADER_LEN: usize = 2;
        let data_len = buffer.len();

        buffer.resize(data_len + HEADER_LEN, 0);
        let (res, mut buffer) = self.read_extend(buffer, HEADER_LEN).await;
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
        let opcode = unsafe { std::mem::transmute::<u8, Opcode>(b1 & 0x0F) };
        let masked = b2 & 0x80 != 0;
        let mut length = (b2 & 0x7F) as usize;

        if rsv != 0 {
            protocol_violation!(self, "Reserve bit must be 0.");
        }
        if masked {
            protocol_violation!(self, "Server to client communication should be unmasked.");
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
                protocol_violation!(self, "Use of reserved opcode.");
            }
            Opcode::Close => {
                if length == 1 {
                    protocol_violation!(self, "Close frame with a missing close reason byte.");
                }
                if length > 125 {
                    protocol_violation!(self, "Control frame larger than 125 bytes.");
                }
                if !fin {
                    protocol_violation!(self, "Control frame cannot be fragmented.");
                }
            }
            Opcode::Ping | Opcode::Pong => {
                if length > 125 {
                    protocol_violation!(self, "Control frame larger than 125 bytes.");
                }
                if !fin {
                    protocol_violation!(self, "Control frame cannot be fragmented.");
                }
            }
            Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                (length, buffer) = match length {
                    126 => {
                        const LENGTH_LEN: usize = 2;
                        buffer.resize(data_len + LENGTH_LEN, 0);
                        let (res, mut buffer) = self.read_extend(buffer, LENGTH_LEN).await;
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
                        let (res, mut buffer) = self.read_extend(buffer, LENGTH_LEN).await;
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

        let (res, buffer) = self.read_extend(buffer, length).await;
        match res {
            Ok(_) => {}
            Err(e) => {
                return (Err(e.into()), buffer);
            }
        }

        let frame = Frame { fin, opcode };
        (Ok(frame), buffer)
    }

    async fn read_extend(&mut self, mut buf: Vec<u8>, len: usize) -> BufResult<usize, Vec<u8>> {
        let offset = buf.len();
        let end = offset + len;

        buf.reserve(len);

        let mut read = 0;
        while read < len {
            let buf_slice = unsafe { SliceMut::new_unchecked(buf, offset + read, end) };
            let (result, buf_slice) = self.stream.read(buf_slice).await;
            buf = buf_slice.into_inner();
            match result {
                Ok(0) => {
                    return (
                        Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "failed to fill whole buffer",
                        )),
                        buf,
                    );
                }
                Ok(n) => {
                    read += n;
                    unsafe { buf.set_init(offset + read) };
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return (Err(e), buf),
            }
        }
        (Ok(read), buf)
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

    pub async fn write_frame(&mut self, frame: Frame, mut buffer: Vec<u8>) -> Result<()> {
        frame.encode_vec(&mut buffer, self.rng.random::<u32>().to_ne_bytes());
        let (res, buffer) = self.stream.write_all(buffer).await;
        match res {
            Ok(_) => {}
            Err(e) => return (Err(e.into()), buffer),
        }
        (Ok(()), buffer)
    }

    async fn write_offset(&mut self, mut buf: Vec<u8>, offset: usize) -> BufResult<usize, Vec<u8>> {
        let len = buf.len() - offset;
        let mut written = 0;
        while written < len {
            let buf_slice = unsafe { Slice::new_unchecked(buf, offset + written, offset + len) };
            let (result, buf_slice) = self.stream.write(buf_slice).await;
            buf = buf_slice.into_inner();
            match result {
                Ok(0) => {
                    return (
                        Err(std::io::Error::new(
                            std::io::ErrorKind::WriteZero,
                            "failed to write whole buffer",
                        )),
                        buf,
                    );
                }
                Ok(n) => written += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return (Err(e), buf),
            }
        }
        (Ok(written), buf)
    }
}
