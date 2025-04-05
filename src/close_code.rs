/// WebSocket close codes as defined in RFC 6455:
/// <https://tools.ietf.org/html/rfc6455#section-7.4>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CloseCode {
    /// 1000 indicates a normal closure, meaning that the purpose for which the
    /// connection was established has been fulfilled.
    Normal = 1000,

    /// 1001 indicates that an endpoint is "going away", such as a server going
    /// down or a browser having navigated away from a page.
    GoingAway = 1001,

    /// 1002 indicates that an endpoint is terminating the connection due to a
    /// protocol error.
    ProtocolError = 1002,

    /// 1003 indicates that an endpoint is terminating the connection because it
    /// has received a type of data it cannot accept. (e.g., an endpoint that
    /// understands only text data MAY send this if it receives a binary
    /// message).
    UnsupportedData = 1003,

    /// 1004 is reserved. The specific meaning might be defined in the future.
    Reserved = 1004,

    /// 1005 is a reserved value and MUST NOT be set as a status code in a Close
    /// control frame by an endpoint. It is designated for use in applications
    /// expecting a status code to indicate that no status code was actually
    /// present.
    NoStatusReceived = 1005,

    /// 1006 is a reserved value and MUST NOT be set as a status code in a
    /// Close control frame by an endpoint. It is designated for use in
    /// applications expecting a status code to indicate that the connection was
    /// closed abnormally, e.g., without sending or receiving a Close control
    /// frame.
    Abnormal = 1006,

    /// 1007 indicates that an endpoint is terminating the connection because
    /// it has received data within a message that was not consistent with the
    /// type of the message (e.g., non-UTF-8 [RFC3629] data within a text
    /// message).
    InvalidFramePayloadData = 1007,

    /// 1008 indicates that an endpoint is terminating the connection because it
    /// has received a message that violates its policy. This is a generic
    /// status code that can be returned when there is no other more suitable
    /// status code (e.g., 1003 or 1009) or if there is a need to hide specific
    /// details about the policy.
    PolicyViolation = 1008,

    /// 1009 indicates that an endpoint is terminating the connection because it
    /// has received a message that is too big for it to process.
    MessageTooBig = 1009,

    /// 1010 indicates that an endpoint (client) is terminating the connection
    /// because it has expected the server to negotiate one or more extension,
    /// but the server didn't return them in the response message of the
    /// WebSocket handshake. The list of extensions that are needed SHOULD
    /// appear in the /reason/ part of the Close frame. Note that this status
    /// code is not used by the server, because it can fail the WebSocket
    /// handshake instead.
    MandatoryExtension = 1010,

    /// 1011 indicates that a server is terminating the connection because it
    /// encountered an unexpected condition that prevented it from fulfilling
    /// the request.
    InternalError = 1011,

    /// 1012 indicates that the service is restarting. A client may reconnect,
    /// and if it chooses to do so, should reconnect using a randomized delay
    /// of 5-30 seconds.
    ServiceRestart = 1012,

    /// 1013 indicates that the service is experiencing overload. A client
    /// should only reconnect using a randomized delay of 5-30 seconds.
    TryAgainLater = 1013,

    /// 1014 indicates that the server was acting as a gateway or proxy and
    /// received an invalid response from the upstream server. This is similar
    /// to 502 HTTP Status Code.
    BadGateway = 1014,

    /// 1015 is a reserved value and MUST NOT be set as a status code in a
    /// Close control frame by an endpoint. It is designated for use in
    /// applications expecting a status code to indicate that the connection was
    /// closed due to a failure to perform a TLS handshake (e.g., the server
    /// certificate can't be verified).
    TlsHandshake = 1015,

    /// 3000-3999: Reserved for use by libraries, frameworks, and applications.
    /// These status codes are registered directly with IANA. The interpretation
    /// of these codes is undefined by the WebSocket protocol.
    Library(u16),

    /// 4000-4999: Reserved for private use. These codes cannot be registered
    /// and the interpretation of these codes is undefined by the WebSocket
    /// protocol.
    Private(u16),
}

impl CloseCode {
    #[must_use]
    pub fn is_reserved(self) -> bool {
        matches!(
            self,
            Self::Reserved | Self::NoStatusReceived | Self::Abnormal
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloseCodeParseError {
    #[error("Invalid WebSocket close code: {0}")]
    InvalidCloseCode(u16),
}

impl TryFrom<u16> for CloseCode {
    type Error = CloseCodeParseError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Ok(match value {
            1000 => Self::Normal,
            1001 => Self::GoingAway,
            1002 => Self::ProtocolError,
            1003 => Self::UnsupportedData,
            1004 => Self::Reserved,
            1005 => Self::NoStatusReceived,
            1006 => Self::Abnormal,
            1007 => Self::InvalidFramePayloadData,
            1008 => Self::PolicyViolation,
            1009 => Self::MessageTooBig,
            1010 => Self::MandatoryExtension,
            1011 => Self::InternalError,
            1012 => Self::ServiceRestart,
            1013 => Self::TryAgainLater,
            1014 => Self::BadGateway,
            1015 => Self::TlsHandshake,
            3000..=3999 => Self::Library(value),
            4000..=4999 => Self::Private(value),
            _ => Err(Self::Error::InvalidCloseCode(value))?,
        })
    }
}

impl From<CloseCode> for u16 {
    fn from(value: CloseCode) -> Self {
        match value {
            CloseCode::Normal => 1000,
            CloseCode::GoingAway => 1001,
            CloseCode::ProtocolError => 1002,
            CloseCode::UnsupportedData => 1003,
            CloseCode::Reserved => 1004,
            CloseCode::NoStatusReceived => 1005,
            CloseCode::Abnormal => 1006,
            CloseCode::InvalidFramePayloadData => 1007,
            CloseCode::PolicyViolation => 1008,
            CloseCode::MessageTooBig => 1009,
            CloseCode::MandatoryExtension => 1010,
            CloseCode::InternalError => 1011,
            CloseCode::ServiceRestart => 1012,
            CloseCode::TryAgainLater => 1013,
            CloseCode::BadGateway => 1014,
            CloseCode::TlsHandshake => 1015,
            CloseCode::Library(code) | CloseCode::Private(code) => code,
        }
    }
}
