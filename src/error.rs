use std::fmt;
use std::fmt::Formatter;

#[derive(Debug)]
pub enum BidError {
    ErrorProtobufEncoding,
    ErrorOnSend,
    ErrorOnRead,
    ProtoParseError,
    PlayerManagerSendFailed,
}

impl std::error::Error for BidError {}

impl fmt::Display for BidError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BidError::ErrorProtobufEncoding => write!(f, "Protobuf Encoding Error."),
            BidError::ErrorOnSend => write!(f, "Error sending data"),
            BidError::ErrorOnRead => write!(f, "Error reading from socket."),
            BidError::ProtoParseError => write!(f, "Error parsing bytes of protobuffer."),
            BidError::PlayerManagerSendFailed => write!(f, "Error sending information to the player manager."),

        }
    }
}