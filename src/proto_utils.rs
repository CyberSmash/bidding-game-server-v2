pub mod proto_utils {
    use crate::protos::Comms;
    use crate::error::BidError;
    use protobuf::{Message, MessageField};
    use async_std::{
        net::{TcpStream},
        prelude::*,

    };
    use log::{warn};

    use std::{
        collections::hash_map::{HashMap, Entry},
    };
    use crate::Player;
    use crate::protos::Comms::server_request::MsgType;

    // @TODO: I'm not sure that this is exactly idiomatic.
    use crate::SOCKET_READ_TIMEOUT;

    /// Sends a ServerRequest to a client.
    ///
    /// # Examples
    /// ```
    /// let req = ServerRequest::new();
    /// req.set_msgType(MsgType::ACK);
    /// send_proto_to_client(&mut player_stream, &req).await;
    /// ```
    pub async fn send_proto_to_client(stream: &mut TcpStream, msg: &Comms::ServerRequest) -> std::result::Result<(), BidError> {
        let final_msg = match  msg.write_length_delimited_to_bytes().map_err(|_| BidError::ErrorProtobufEncoding) {
            Ok(m) => {m}
            Err(e) => {
                println!("[-] I ran into an error encoding protobuffer.");
                return Err(e);
            }
        };

        match stream.write_all(final_msg.as_slice()).await.map_err(|_| BidError::ErrorOnSend) {
            Ok(_) => {}
            Err(e) => {
                println!("[-] Error writing proto buffer.");
                return Err(e);
            }
        };
        Ok(())
    }

    /// Reads a varint from the stream.
    ///
    /// This is used to be able to tell how big the message being sent is. For more information
    /// on varints see: https://en.wikipedia.org/wiki/Variable-length_quantity
    ///
    async fn read_varint_from_stream(mut stream: &TcpStream) -> std::result::Result<i32, BidError> {
        let mut result: i32 = 0;
        let mut shift = 0;
        let mut buffer = vec![0u8; 1];
        loop {
            match async_std::io::timeout(SOCKET_READ_TIMEOUT, stream.read_exact(&mut buffer)).await
            {
                Ok(_) => {}
                Err(e) => {
                    warn!("Error reading varint: {:?}", e);
                    return Err(BidError::ErrorOnRead);
                }
            };
            result |= ((buffer[0] & 0x7F) << shift) as i32;
            shift += 7;

            if !(buffer[0] & 0x80 > 0) {
                break;
            }
        }
        Ok(result)
    }

    /// Read a ServerRequest protobuf from the client.
    ///
    /// TODO: This should probably be renamed to move away from the word 'proto'.
    ///
    /// # Example
    /// ```
    /// let msg = match read_proto_from_client(&mut player_stream).await {
    ///     Ok(m) => m,
    ///     Err(e) => {
    ///         println!("Error: {:?}", e);
    ///         // ...
    ///     }
    /// }
    /// ```
    pub async fn read_proto_from_client(mut stream: &TcpStream) -> std::result::Result<Comms::ServerRequest, BidError> {
        // Get the size of the data first.
        let num_bytes = read_varint_from_stream(stream).await?;

        if num_bytes <= 0 {
            return Err(BidError::ErrorOnRead);
        }

        let mut raw_data = vec![0u8; num_bytes as usize];

        // Read in the bytes from the stream
        async_std::io::timeout(SOCKET_READ_TIMEOUT, stream.read_exact(&mut raw_data))
            .await
            .map_err(|_| BidError::PlayerTimedOut)?;

        let server_request =  Comms::ServerRequest::parse_from_bytes(
            raw_data.as_slice())
            .map_err(|_| BidError::ProtoParseError)?;
        Ok(server_request)
    }

    /// Send a ServerRequest to a player by name.
    ///
    /// This is a specialty helper function when finding the player's stream object is inconvenient.
    ///
    /// Note: I guess this COULD go into player management, mayber there's a better place for this.
    pub async fn send_proto_to_client_by_name(players: &mut HashMap<String, Player>, player_name: &String, msg: Comms::ServerRequest) -> std::result::Result<(), BidError> {
        return match players.entry(player_name.to_string()) {
            Entry::Occupied(mut player_entry) => {
                let player = player_entry.get_mut();
                send_proto_to_client(&mut player.stream, &msg).await?;
                Ok(())
            }
            Entry::Vacant(_) => {
                println!("I can't find the player {} to send a message to.", player_name);
                Err(BidError::PlayerNotFoundByName)
            }
        }
    }

    /// Creates a start proto.
    ///
    /// This is just a simple factory function for creating a GAME_START message as they can get
    /// a bit verbose when inlined.
    pub fn make_start_proto(player_a_name: String, player_a_money: u32, player_b_name: String, player_b_money: u32) -> Comms::ServerRequest
    {
        let mut start_msg = Comms::ServerRequest::new();
        start_msg.set_msgType(Comms::server_request::MsgType::GAME_START);

        let mut gs = Comms::GameStart::new();
        //start_msg.

        gs.set_player1_name(player_a_name);
        gs.set_player2_name(player_b_name);
        gs.set_player1_start_money(player_a_money);
        gs.set_player2_start_money(player_b_money);
        start_msg.gameStart = MessageField::some(gs);
        return start_msg;
    }

    /// Creates a round start protobuffer.
    ///
    /// This is a helper factory function for making ROUND_START protos
    /// as these can be a bit verbose when inlined.
    pub fn make_round_start_proto(round_num: u32, player_a_money: u32, player_b_money: u32, bottle_pos: u32) -> Comms::ServerRequest {
        let mut round_msg = Comms::ServerRequest::new();
        round_msg.set_msgType(MsgType::ROUND_START);
        let mut round_start = Comms::RoundStart::new();
        round_start.set_round_num(round_num);
        round_start.set_player_a_money(player_a_money);
        round_start.set_player_b_money(player_b_money);
        round_start.set_bottle_pos(bottle_pos);

        round_msg.roundStart = MessageField::some(round_start);

        return round_msg;
    }


}