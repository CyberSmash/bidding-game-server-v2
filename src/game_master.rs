pub mod game_master {
    use crate::protos::Comms;
    //use crate::protos::ServerRequest;
    use crate::protos::Comms::server_request::MsgType;
    use protobuf::{Message, MessageField};

    pub fn craft_game_end_proto(result :Comms::game_end::GameResult) -> Comms::ServerRequest
    {
        let mut msg = Comms::ServerRequest::new();
        let mut game_end = Comms::GameEnd::new();

        msg.set_msgType(MsgType::GAME_END);
        game_end.set_result(result);
        msg.gameEnd = MessageField::some(game_end);

        return msg;

    }
}