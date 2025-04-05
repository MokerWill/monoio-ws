/// WebSocket Opcodes as defined in RFC 6455:
/// <https://datatracker.ietf.org/doc/html/rfc6455#section-11.8>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    /// Continuation frame.
    Continuation = 0x0,

    /// Text frame.
    Text = 0x1,

    /// Binary frame.
    Binary = 0x2,

    /// Reserved for future non-control frames.
    Reserved3 = 0x3,

    /// Reserved for future non-control frames.
    Reserved4 = 0x4,

    /// Reserved for future non-control frames.
    Reserved5 = 0x5,

    /// Reserved for future non-control frames.
    Reserved6 = 0x6,

    /// Reserved for future non-control frames.
    Reserved7 = 0x7,

    /// Connection close.
    Close = 0x8,

    /// Ping.
    Ping = 0x9,

    /// Pong.
    Pong = 0xA,

    /// Reserved for future control frames.
    ReservedB = 0xB,

    /// Reserved for future control frames.
    ReservedC = 0xC,

    /// Reserved for future control frames.
    ReservedD = 0xD,

    /// Reserved for future control frames.
    ReservedE = 0xE,

    /// Reserved for future control frames.
    ReservedF = 0xF,
}

impl Opcode {
    /// Returns true if this is a control frame (8-15).
    #[must_use]
    pub fn is_control(self) -> bool {
        (self as u8) & 0x8 == 0x8
    }

    /// Returns true if this is a data frame (0-2).
    #[must_use]
    pub fn is_data(self) -> bool {
        matches!(self, Self::Continuation | Self::Text | Self::Binary)
    }

    /// Returns true if this is a reserved opcode.
    #[must_use]
    pub fn is_reserved(self) -> bool {
        !self.is_data() && !matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpcodeParseError {
    #[error("Invalid WebSocket opcode: {0}")]
    InvalidOpcode(u8),
}

impl TryFrom<u8> for Opcode {
    type Error = OpcodeParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x0 => Ok(Self::Continuation),
            0x1 => Ok(Self::Text),
            0x2 => Ok(Self::Binary),
            0x3 => Ok(Self::Reserved3),
            0x4 => Ok(Self::Reserved4),
            0x5 => Ok(Self::Reserved5),
            0x6 => Ok(Self::Reserved6),
            0x7 => Ok(Self::Reserved7),
            0x8 => Ok(Self::Close),
            0x9 => Ok(Self::Ping),
            0xA => Ok(Self::Pong),
            0xB => Ok(Self::ReservedB),
            0xC => Ok(Self::ReservedC),
            0xD => Ok(Self::ReservedD),
            0xE => Ok(Self::ReservedE),
            0xF => Ok(Self::ReservedF),
            invalid => Err(Self::Error::InvalidOpcode(invalid)),
        }
    }
}

impl From<Opcode> for u8 {
    fn from(value: Opcode) -> Self {
        value as Self
    }
}
