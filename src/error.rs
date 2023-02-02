use std::fmt;
//use std::fmt::Formatter;

#[derive(Debug)]
pub enum BidError {
    ErrorBindingToInterface,
    ErrorProtobufEncoding,
    ErrorOnSend,
    ErrorOnRead,
    ProtoParseError,
    PlayerManagerSendFailed,
    PlayerNotFoundByName,
    MaxBidErrorsReached,
    UnexpectedResponseTypeFromClient,
    PlayerTimedOut,
    DbCannotPrepStatement,
    DbCannotOpenDatabase,
    PlayerAAbandoned,
    PlayerBAbandoned,
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
            BidError::PlayerNotFoundByName => write!(f, "Could not find the player by their name in our players list."),
            BidError::MaxBidErrorsReached => write!(f, "Maximum number of client bid errors reached."),
            BidError::UnexpectedResponseTypeFromClient => write!(f, "Unexpected response type received from the client."),
            BidError::PlayerTimedOut => write!(f, "The player has timed out."),
            BidError::DbCannotOpenDatabase => write!(f, "Could not open database file."),
            BidError::DbCannotPrepStatement => write!(f, "Could not prepare statement for database."),
            BidError::ErrorBindingToInterface => write!(f, "Error binding interface / port."),
            BidError::PlayerAAbandoned => write!(f, "Player A abandoned the game."),
            BidError::PlayerBAbandoned => write!(f, "Player B abandoned the game."),

        }
    }
}