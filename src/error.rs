use std::fmt;
use std::fmt::Formatter;

#[derive(Debug)]
pub enum BidError {
    ErrorProtobufEncoding,
    ErrorOnSend,
}

impl std::error::Error for BidError {}

impl fmt::Display for BidError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            BidError::ErrorProtobufEncoding => {write!(f, "Protobuf Encoding Error.")}
            BidError::ErrorOnSend => {write!(f, "Error sesnding data")}
        }
    }
}