pub mod game_master {
    use std::thread;
    use std::time::Duration;
    use futures::join;
    use async_std::net::{TcpStream};
    use async_std::prelude::*;
    use futures::channel::mpsc;
    use log::{error, info, trace, warn};
    use crate::protos::Comms;
    //use crate::protos::ServerRequest;
    use crate::protos::Comms::server_request::MsgType;
    use futures::sink::SinkExt;
    use protobuf::{MessageField};
    use crate::error::BidError;
    use crate::{BOTTLE_MAX, BOTTLE_MIN, Event, GameResult, GameResultType, GameStatus, MAX_BID_ERRORS, MAX_GAME_ROUNDS};
    use crate::proto_utils::proto_utils::{make_round_start_proto, make_start_proto, read_proto_from_client, send_proto_to_client};
    use crate::protos::Comms::bid_result::RoundResultType;
    use crate::protos::Comms::ServerRequest;

    type Sender<T> = mpsc::UnboundedSender<T>;
    type Receiver<T> = mpsc::UnboundedReceiver<T>;

    pub fn craft_game_end_proto(result: Comms::game_end::GameResult) -> Comms::ServerRequest
    {
        let mut msg = Comms::ServerRequest::new();
        let mut game_end = Comms::GameEnd::new();

        msg.set_msgType(MsgType::GAME_END);
        game_end.set_result(result);
        msg.gameEnd = MessageField::some(game_end);

        return msg;
    }


    async fn get_bid_from_client(stream: &mut TcpStream, player_money_left: u32) -> std::result::Result<u32, BidError> {
        let mut error_count = 0;
        let mut bad_bid = ServerRequest::new();
        bad_bid.set_msgType(MsgType::BID_REJECT);

        let mut ack_bid = ServerRequest::new();
        ack_bid.set_msgType(MsgType::ACK);


        let mut bid_request_msg = Comms::ServerRequest::new();
        bid_request_msg.set_msgType(Comms::server_request::MsgType::BID_REQUEST);

        while error_count < MAX_BID_ERRORS {
            send_proto_to_client(stream, &bid_request_msg).await?;

            let bid_num = match read_proto_from_client(stream).await {
                Ok(response_proto) => {
                    let bid_amount: u32;
                    if response_proto.msgType() == MsgType::BID_RESPONSE {
                        bid_amount = match response_proto.bidResponse.money {
                            None => {
                                println!("Could not get the money field from our bid response.");
                                error_count += 1;
                                continue;
                            }
                            Some(amount) => amount
                        };
                    } else {
                        println!("Error: I did not receive the expected bid response.");
                        error_count += 1;
                        continue;
                    }
                    bid_amount
                }
                Err(e) => {
                    match e {
                        BidError::ErrorProtobufEncoding | BidError::ProtoParseError => {
                            error_count += 1;
                            println!("Error decoding protobuf.");
                            continue;
                        }
                        BidError::ErrorOnRead => {
                            // In this case the client has likely closed the connection. They get no
                            // more chances.
                            println!("Client has gone away while getting a bid.");
                            return Err(e);
                        }
                        BidError::PlayerTimedOut => {
                            error_count += 1;
                            println!("Player timed out waiting for a bid. They have {} chances left", 3 - error_count);
                            continue;
                        }
                        _ => {
                            println!("Unexpected error.");
                            return Err(e);
                        }
                    }
                }
            };

            // You can't bid more than you have, and you cannot bid 0 unless you only
            // have 0 to bid.
            if (bid_num > player_money_left) || (bid_num == 0 && player_money_left > 0) {
                error_count += 1;
                send_proto_to_client(stream, &bad_bid).await?;
                continue;
            } else {
                send_proto_to_client(stream, &ack_bid).await?;
                return Ok(bid_num);
            }
        }
        return Err(BidError::MaxBidErrorsReached);
    }


    async fn run_game(_id: u32, stream_a: &mut TcpStream, name_a: &String,
                      stream_b: &mut TcpStream, name_b: &String) -> std::result::Result<GameResultType, BidError> {
        let mut player_a_money_left = 100;
        let mut player_b_money_left = 100;
        let mut bottle_position: u32 = 5;
        let mut draw_advantage = true;


        let start_msg = make_start_proto(name_a.clone(),
                                         player_b_money_left,
                                         name_b.clone(),
                                         player_b_money_left);


        let results = join!(send_proto_to_client(stream_a, &start_msg),
                                             send_proto_to_client(stream_b, &start_msg));

        if results.0.is_err() {
            return Err(BidError::PlayerAAbandoned);
        }
        if results.1.is_err() {
            return Err(BidError::PlayerBAbandoned);
        }

        for round in 0..MAX_GAME_ROUNDS {
            let round_start_msg = make_round_start_proto(round + 1, player_a_money_left, player_b_money_left, bottle_position);

            let results = join!(send_proto_to_client(stream_a, &round_start_msg), send_proto_to_client(stream_b, &round_start_msg));

            if results.0.is_err() {
                return Err(BidError::PlayerAAbandoned);
            }
            if results.1.is_err() {
                return Err(BidError::PlayerBAbandoned)
            }

            if bottle_position == BOTTLE_MIN || bottle_position == BOTTLE_MAX {
                return if bottle_position == BOTTLE_MIN {
                    Ok(GameResultType::PlayerBWins)
                } else {
                    Ok(GameResultType::PlayerAWins)
                }
            }

            let bids = join!(get_bid_from_client(stream_a, player_a_money_left),
            get_bid_from_client(stream_b, player_b_money_left));

            let bid_a = bids.0.map_err(|_| BidError::PlayerAAbandoned)?;
            let bid_b = bids.1.map_err(|_| BidError::PlayerBAbandoned)?;


            let mut winner_name = String::new();
            if bid_a > bid_b {
                player_a_money_left -= bid_a;
                bottle_position += 1;
                winner_name = name_a.clone();
            } else if bid_a < bid_b {
                player_b_money_left -= bid_b;
                bottle_position -= 1;
                winner_name = name_b.clone();
            } else {
                if draw_advantage {
                    // Player A draw advantage
                    bottle_position += 1;
                    player_a_money_left -= bid_a;
                } else {
                    // Player B draw advantage
                    bottle_position -= 1;
                    player_b_money_left -= bid_b;
                }
                draw_advantage = !draw_advantage;
            }

            let mut bid_result = ServerRequest::new();
            bid_result.set_msgType(MsgType::BID_RESULT);
            let mut br: Comms::BidResult = Comms::BidResult::new();
            br.set_player_a_bid(bid_a);
            br.set_player_b_bid(bid_b);
            br.set_winner_name(winner_name);
            br.set_result_type(if bid_a != bid_b { RoundResultType::WIN } else { RoundResultType::DRAW });
            bid_result.bidResult = MessageField::some(br);

            send_proto_to_client(stream_a, &bid_result).await.map_err(|_| BidError::PlayerAAbandoned)?;
            send_proto_to_client(stream_b, &bid_result).await.map_err(|_| BidError::PlayerBAbandoned)?
        } // end for-loop

        trace!("The game has finished and we are returning the result.");
        Ok(GameResultType::Draw)
    }

    pub fn create_game_result_abandoned(id: u32, name_a: String, name_b: String, e: BidError) -> GameResult {
        let mut gr = GameResult {
            id,
            winner: "".to_string(),
            loser: "".to_string(),
            status: GameStatus::Abandoned,
        };

        gr.winner = if matches!(e, BidError::PlayerAAbandoned) {name_b.clone()} else {name_a.clone()};
        gr.loser = if matches!(e, BidError::PlayerAAbandoned) {name_a.clone()} else {name_b.clone()};
        match e {
            BidError::PlayerAAbandoned => {
                warn!("It looks like player {} disconnected before the game could be finished.", name_a);
                gr.winner = name_b;
                gr.loser = name_a;
            }
            BidError::PlayerBAbandoned => {
                warn!("It looks like player {} disconnected before the game could be finished.", name_b);
                gr.winner = name_a;
                gr.loser = name_b;
            }
            _ => {
                println!("Unexpected error from run_game: {:?}", e);
                // TODO: This should do something with the game result to indicate
                // an unknown error.
            }
        }

        gr
    }

    pub async fn handle_player_winning_result(id: u32, name_a: String, stream_a: &mut TcpStream,
                                              name_b: String, stream_b: &mut TcpStream,
                                              game_result: GameResultType) -> Result<GameResult, BidError> {
        let mut gr = GameResult {
            id,
            winner: "".to_string(),
            loser: "".to_string(),
            status: GameStatus::Completed,
        };
        let winner_message = craft_game_end_proto(Comms::game_end::GameResult::WIN);
        let loser_message = craft_game_end_proto(Comms::game_end::GameResult::LOSS);

        let results;
        warn!("GM: {} We have a winning scenario. Returning result.", id);
        if matches!(game_result, GameResultType::PlayerAWins) {
            results = join!(send_proto_to_client(stream_a, &winner_message),
                                send_proto_to_client(stream_b, &loser_message));
            gr.winner = name_a;
            gr.loser = name_b;
        }
        else {
            results = join!(send_proto_to_client(stream_b, &winner_message),
                                send_proto_to_client(stream_a, &loser_message));
            gr.winner = name_b;
            gr.loser = name_a;
        }


        match results.0 {
            Err(e) => {
                warn!("I lost player_a -- Reason {}", e);
                return Err(e);
            }
            Ok(_) => {}

        }

        match results.1 {
            Err(e) => {
                warn!("I lost player_b -- Reason: {}", e);
                return Err(e);
            }
            Ok(_) => {}
        }

        Ok(gr)
    }



    pub async fn game_master(mut pm_sender: Sender<Event>, mut gm_receiver: Receiver<Event>, id: u32)
    {
        let gm_backoff = Duration::from_millis(25);


        info!("Game Master: {} is alive!", id);

        pm_sender.send(Event::NeedPlayers {id}).await.unwrap_or_else(|error| {
            error!("Could not contact the player manager {}. Exiting.", error);
            return;
        });
        while let Some(event) = gm_receiver.next().await {

            let mut gr = GameResult {
                id,
                winner: "".to_string(),
                loser: "".to_string(),
                status: GameStatus::Completed,
            };

            match event {
                Event::NewGame { name_a, name_b, mut stream_a, mut stream_b } => {
                    info!("GM: {} Got new players {} {}", id, name_a, name_b);
                    let game_result = match run_game(id, &mut stream_a, &name_a, &mut stream_b, &name_b).await {
                        Ok(gr) => gr,
                        Err(e) => {

                            gr = create_game_result_abandoned(id, name_a, name_b, e);


                            // Note: It's necessary to handle an error here, as opposed to after the match statement
                            // later. Don't delete this code, even though it's repeated.
                            pm_sender.send(Event::GameOver { result: gr }).await.unwrap_or_else(|error| {
                                error!("Lost contact to the player manager: {}", error);
                                return;
                            });
                            thread::sleep(gm_backoff);
                            pm_sender.send(Event::NeedPlayers {id}).await.unwrap_or_else(|error| {
                                error!("Cannot contact the player manager: {}", error);
                                return;
                            });
                            continue;

                        }
                    };
                    match game_result {
                        GameResultType::PlayerAWins | GameResultType::PlayerBWins => {

                            let result = handle_player_winning_result(id, name_a, &mut stream_a,
                                                         name_b, &mut stream_b,
                                                         game_result).await;

                            match result {
                                Ok(game_result) =>
                                    {
                                        gr = game_result;
                                    },
                                Err(e) => {
                                    error!("Error: {}", e);
                                }
                            }
                        }
                        GameResultType::Draw => {
                           let draw_msg = craft_game_end_proto(Comms::game_end::GameResult::DRAW);

                            let results = join!(send_proto_to_client(&mut stream_a, &draw_msg),
                            send_proto_to_client(&mut stream_b, &draw_msg)
                            );
                            if results.0.is_err() {
                                warn!("Could not send proto to client A.");
                            }
                            if results.1.is_err() {
                                warn!("Could not send proto to client B")
                            }
                            gr.winner = name_a;
                            gr.loser = name_b;
                            gr.status = GameStatus::Draw;
                        }
                    }
                    info!("GM {} the game is over and I'm returning the results.", id);
                    pm_sender.send(Event::GameOver { result: gr }).await.unwrap_or_else(|err| error!("Error sending to the player manager. {:?}", err));
                    thread::sleep(gm_backoff);
                    pm_sender.send(Event::NeedPlayers {id}).await.unwrap_or_else(|err| error!("Error sending to the player manager. {:?}", err));

                }
                Event::NoPlayersAvailable => {
                    thread::sleep(gm_backoff);
                    pm_sender.send(Event::NeedPlayers {id}).await.unwrap_or_else(|err| error!("Error sending to the player manager. {:?}", err))
                }
                _ => {}
            } // match event
        } // while loop
    }

}