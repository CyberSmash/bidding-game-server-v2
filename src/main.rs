use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
};

use sqlx::sqlite::{SqlitePoolOptions, Sqlite, SqlitePool};
use std::{thread};
use futures::{join, StreamExt};

use futures::channel::mpsc;
use futures::sink::SinkExt;
use std::{
    collections::hash_map::{HashMap, Entry},
};

use std::net::Shutdown;
use chrono::{Utc};
use std::time::Duration;
use protobuf::{Message, MessageField};
use rand::{rngs::StdRng, Rng, SeedableRng};
mod protos;
mod player_management;
use clap::{Parser};
use protos::Comms;
use std::path::PathBuf;
mod error;
use simple_logger;
use error::BidError;
use crate::Comms::ServerRequest;
use crate::protos::Comms::bid_result::RoundResultType;
use crate::protos::Comms::server_request::MsgType;
use log::{warn, info, trace, error};
mod proto_utils;
mod game_master;

use proto_utils::proto_utils::*;
use player_management::player_management::*;
use futures::channel::mpsc::SendError;
use crate::game_master::game_master::craft_game_end_proto;
use crate::GameResultType::PlayerAWins;

type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;
//type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;


const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_GAME_ROUNDS: u32 = 10;
const BOTTLE_MIN: u32 = 0;
const BOTTLE_MAX: u32 = 10;
const MAX_BID_ERRORS: u32 = 3;


enum GameStatus {
    Completed,
    Draw,
    Abandoned,
}

enum Event {
    NewPlayer {
        player: Player
    },
    NewGame {
        name_a: String,
        name_b: String,
        stream_a: TcpStream,
        stream_b: TcpStream,
    },
    GameOver {
        result: GameResult,
    },
    NeedPlayers {
        id: u32
    },
    NoPlayersAvailable
}

struct GameMasterInfo {
    id: u32,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
}

enum GameResultType {
    PlayerAWins,
    PlayerBWins,
    Draw,
}

struct GameResult
{
    // The ID of the game master.
    id: u32,
    // The winner of the game if there is one
    winner: String,
    // The loser of the game if there is one
    loser: String,
    // The result of the game.
    status: GameStatus,
}

#[derive(sqlx::FromRow)]
struct DatabaseUser {
    id: i32,
    username: String,
    token: i32,
    wins: i32,
    losses: i32,
    elo: i32,
    draws: i32,
}

// The player managers representation of a player. This is the authoritative
// copy of the player information.
pub struct Player {
    id: i32,
    name: String,
    wins: u64,
    losses: u64,
    in_game: bool,
    stream: TcpStream,
    player_cooldown: chrono::DateTime<Utc>
}

// Only the info that the Game Master needs of a player.
pub struct GMPlayer {
    name: String,
    stream: TcpStream,
}

type SomeResult<T> = std::result::Result<T, BidError>;

/**
 * Arguments Parsing.
 */
#[derive(Parser, Debug)]
struct Args {

    #[arg(short, long, value_name = "FILE")]
    database: PathBuf,

    #[arg(short, long, default_value_t = String::from("localhost"))]
    interface: String,

    #[arg(short, long, default_value_t = String::from("8080"))]
    port: String
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
                        Some(amount) => {
                            amount
                        }
                    };
                }
                else {
                    println!("Error: I did not receive the expected bid response.");
                    return Err(BidError::UnexpectedResponseTypeFromClient);
                }
                bid_amount
            }
            Err(e) => {
                match e {
                    BidError::ErrorProtobufEncoding => {
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
                    BidError::ProtoParseError => {
                        error_count += 1;
                        println!("Could not parse protobuf.");
                        continue;
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
        }
        else {
            send_proto_to_client(stream, &ack_bid).await?;
            return Ok(bid_num);
        }
    }
    return Err(BidError::MaxBidErrorsReached);
}


async fn run_game(id: u32, stream_a: &mut TcpStream, name_a: &String,
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
                //gr.winner = name_b.clone();
                //gr.loser = name_a.clone();
                Ok(GameResultType::PlayerBWins)
            } else {
                //gr.winner = name_a.clone();
                //gr.loser = name_b.clone();
                Ok(GameResultType::PlayerAWins)
            }
        }

        let bids = join!(get_bid_from_client(stream_a, player_a_money_left),
            get_bid_from_client(stream_b, player_b_money_left));

        let bid_a = bids.0.map_err(|x| BidError::PlayerAAbandoned)?;
        let bid_b = bids.1.map_err(|x| BidError::PlayerBAbandoned)?;


        let mut winner_name = String::new();
        if bid_a > bid_b {
            player_a_money_left -= bid_a;
            bottle_position += 1;
            winner_name = name_a.clone();

        }
        else if bid_a < bid_b {
            player_b_money_left -= bid_b;
            bottle_position -= 1;
            winner_name = name_b.clone();
        }
        else {
            if draw_advantage {
                bottle_position += 1;
            }
            else {
                bottle_position -= 1;
            }
            draw_advantage = !draw_advantage;
        }

        let mut bid_result = ServerRequest::new();
        bid_result.set_msgType(MsgType::BID_RESULT);
        let mut br: Comms::BidResult = Comms::BidResult::new();
        br.set_player_a_bid(bid_a);
        br.set_player_b_bid(bid_b);
        br.set_winner_name(winner_name);
        br.set_result_type(if bid_a != bid_b { RoundResultType::WIN} else {RoundResultType::DRAW});
        bid_result.bidResult = MessageField::some(br);

        send_proto_to_client(stream_a, &bid_result).await.map_err(|x| BidError::PlayerAAbandoned)?;
        send_proto_to_client(stream_b, &bid_result).await.map_err(|x| BidError::PlayerBAbandoned)?


    } // end for-loop

    println!("The game has finished and we are returning the result.");
    Ok(GameResultType::Draw)
}

async fn game_master(mut pm_sender: Sender<Event>, mut gm_receiver: Receiver<Event>, id: u32)
{
    let gm_backoff = Duration::from_millis(25);


    println!("Game Master: {} is alive!", id);

    pm_sender.send(Event::NeedPlayers {id}).await;
    while let Some(event) = gm_receiver.next().await {

        let mut gr = GameResult {
            id,
            winner: "".to_string(),
            loser: "".to_string(),
            status: GameStatus::Completed
        };

        match event {
            Event::NewGame { name_a, name_b, mut stream_a, mut stream_b } => {
                println!("GM: {} Got new players {} {}", id, name_a, name_b);
                let game_result = match run_game(id, &mut stream_a, &name_a, &mut stream_b, &name_b).await {
                    Ok(gr) => gr,
                    Err(e) => {
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
                                println!("It looks like player {} disconnected before the game could be finished.", name_a);
                                gr.winner = name_b;
                                gr.loser = name_a;
                            }
                            BidError::PlayerBAbandoned => {
                                println!("It looks like player {} disconnected before the game could be finished.", name_b);
                                gr.winner = name_a;
                                gr.loser = name_b;
                            }
                            _ => {
                                println!("Unexpected error from run_game: {:?}", e);
                                // TODO: This should do something with the game result to indicate
                                // an unknown error.
                            }
                        }

                        // TODO: Handle these possible problems.
                        pm_sender.send(Event::GameOver { result: gr }).await;
                        thread::sleep(gm_backoff);
                        pm_sender.send(Event::NeedPlayers {id}).await;
                        continue;
                    }
                };
                match game_result {
                    GameResultType::PlayerAWins | GameResultType::PlayerBWins => {

                        let mut winner_message = craft_game_end_proto(Comms::game_end::GameResult::WIN);
                        let mut loser_message = craft_game_end_proto(Comms::game_end::GameResult::LOSS);

                        let mut results;
                        warn!("GM: {} We have a winning scenario. Returning result.", id);
                        if matches!(game_result, GameResultType::PlayerAWins) {
                            results = join!(send_proto_to_client(&mut stream_a, &winner_message),
                                send_proto_to_client(&mut stream_b, &loser_message));
                            gr.winner = name_a;
                            gr.loser = name_b;
                            gr.status = GameStatus::Completed;
                        }
                        else {
                            results = join!(send_proto_to_client(&mut stream_b, &winner_message),
                                send_proto_to_client(&mut stream_a, &loser_message));
                            gr.winner = name_b;
                            gr.loser = name_a;
                            gr.status = GameStatus::Completed;
                        }


                        results.0.unwrap_or_else(|err| {
                            // @TODO: We clearly need a way to tell the player manager
                            // in a better way that a player has disconnected.
                            warn!("I lost player_a")
                        });

                        results.1.unwrap_or_else(|err| {
                            // @TODO: We clearly need a way to tell the player manager
                            // in a better way that a player has disconnected.
                            warn!("I lost player_b")
                        });
                    }
                    GameResultType::Draw => {
                        let mut draw_msg = ServerRequest::new();
                        draw_msg.set_msgType(MsgType::GAME_END);
                        let mut ge = Comms::GameEnd::new();
                        ge.set_result(Comms::game_end::GameResult::DRAW);
                        draw_msg.gameEnd = MessageField::some(ge);
                        let results = join!(send_proto_to_client(&mut stream_a, &draw_msg),
                            send_proto_to_client(&mut stream_b, &draw_msg));
                        gr.winner = name_a;
                        gr.loser = name_b;
                        gr.status = GameStatus::Draw;
                    }
                }
                warn!("GM {} the game is over and I'm returning the results.", id);
                pm_sender.send(Event::GameOver { result: gr }).await.unwrap_or_else(|err| println!("Error sending to the player manager. {:?}", err));
                thread::sleep(gm_backoff);
                pm_sender.send(Event::NeedPlayers {id}).await.unwrap_or_else(|err| println!("Error sending to the player manager. {:?}", err));

            }
            Event::NoPlayersAvailable => {
                thread::sleep(gm_backoff);
                pm_sender.send(Event::NeedPlayers {id}).await.unwrap_or_else(|err| println!("Error sending to the player manager. {:?}", err))
            }
            _ => {}
        } // match event
    } // while loop
}


async fn handle_need_players(players: &mut HashMap<String, Player>, gm_sender: &mut Sender<Event>) -> bool {
    let free_players = get_free_players(players).await;

    if free_players.len() < 2 {
        match gm_sender.send(Event::NoPlayersAvailable).await
        {
            Ok(_) => {}
            Err(e) => {
                println!("Error sending message to game master. {:?}", e);
            }
        }
        return false;
    }
    let player_a = match get_player_info_for_game(players, &free_players[0])
    {
        None => {
            println!("I errored out trying to find a player. The player {} was suddenly not found.", free_players[0]);
            match gm_sender.send(Event::NoPlayersAvailable).await {
                Ok(_) => {}
                Err(e) => {
                    // There's not much we can do about this. Just return and hope for the best.
                    // There may be a case to return an error instead of a boolean.
                    println!("I couldn't talk to the game manager due to error: {}", e);
                }
            };
            return false;
        }
        Some(p) => {p}
    };

    let player_b = match get_player_info_for_game(players, &free_players[1])
    {
        None => {
            println!("I errored out trying to find a player. The player {} was suddenly not found.", free_players[1]);
            return false;
        }
        Some(p) => {p}
    };

    println!("Sending {} and {} to start a game", player_a.name, player_b.name);
    match gm_sender.send(Event::NewGame {
        name_a: player_a.name,
        name_b: player_b.name,
        stream_a: player_a.stream.clone(),
        stream_b: player_b.stream.clone(),
    }).await
    {
        Ok(_) => {}
        Err(e) => {
            println!("Error: Something went wrong sending the new game message to the \
                        game manager. E: {}", e.to_string());
        }
    }

    true
}

async fn player_manager(sender: Sender<Event>, mut receiver: Receiver<Event>, database_pool: SqlitePool) -> Result<(), BidError> {


    let mut rng = StdRng::seed_from_u64(Utc::now().timestamp() as u64);
    let mut players: HashMap<String, Player> = HashMap::new();
    let mut game_comms: HashMap<u32, Sender<Event>> = HashMap::new();
    let mut db_connection = match database_pool.acquire().await {
        Ok(c) => c,
        Err(e) => {
            println!("Could not acquire a connection to the database from the pool.");
            return Err(BidError::DbCannotOpenDatabase);
        }
    };
    let max_games = 4;

    for id in 0..max_games {
        let (gm_sender, gm_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();
        game_comms.insert(id.clone(), gm_sender);
        task::spawn(game_master(sender.clone(), gm_receiver, id));
    }


    while let Some(event) = receiver.next().await {
        match event {
            Event::NewPlayer { player } => {
                println!("The player manager has recvd. {}", player.name);
                players.insert(player.name.clone(), player);
                println!("Num players: {}", players.len())
            }
            Event::GameOver {
                result
            } => {
                match result.status {
                    GameStatus::Abandoned => {
                        println!("The game had to be abandoned due to {}", result.loser);
                        let mut abort_msg = ServerRequest::new();
                        abort_msg.set_msgType(MsgType::GAME_ABORT);

                        println!("The player manager has been told that {} disconnected.", result.loser);
                        players[&result.loser].stream.shutdown(Shutdown::Both).ok();
                        match send_proto_to_client_by_name(&mut players, &result.winner, abort_msg).await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                match e {
                                    BidError::PlayerNotFoundByName => {
                                    }
                                    _ => {}

                                }
                            }
                        };
                        players.remove(&result.loser);
                    }
                    GameStatus::Completed => {
                        println!("The game completed with {} as the winner, and {} as the loser",
                                 result.winner,
                                 result.loser)
                    }
                    GameStatus::Draw => {
                        sqlx::query("UPDATE players SET wins = draws + 1 where username = ? or username = ?")
                            .bind(result.winner.clone())
                            .bind(result.loser.clone())
                            .execute(&mut *db_connection).await.unwrap();
                    }
                }
                if let Some(peer) = players.get_mut(&result.winner) {
                    println!("Player {} has returned to the pool.", result.winner);
                    peer.player_cooldown = Utc::now();// + chrono::Duration::seconds(rng.gen_range(1..10));
                    peer.in_game = false;
                    peer.wins += 1;
                    sqlx::query("UPDATE players SET wins = wins + 1 where username = ?").bind(result.winner).execute(&mut *db_connection).await.unwrap();

                }
                if let Some(peer) = players.get_mut(&result.loser)
                {
                    println!("Player {} has been returned to the pool.", result.loser);
                    peer.player_cooldown = Utc::now(); // + chrono::Duration::seconds(rng.gen_range(1..10));
                    peer.in_game = false;
                    peer.losses += 1;
                    sqlx::query("UPDATE players SET wins = losses + 1 where username = ?").bind(result.loser).execute(&mut *db_connection).await.unwrap();
                }
            }
            Event::NeedPlayers {id} => {

                let gm_sender = match game_comms.get_mut(&id)
                {
                    None => {
                        println!("So I shouldn't be here but I lost the game master with the ID of {}", id);
                        continue;
                    }
                    Some(gm) => {gm}
                };
                handle_need_players(&mut players, gm_sender).await;

            }
            _ => {}
        }
    }

    Ok(())

}

async fn connection_loop(mut stream: TcpStream, mut pm_sender: Sender<Event>, database_connection: SqlitePool) -> std::result::Result<(), BidError>
{
    let mut auth_request = Comms::ServerRequest::new();
    auth_request.set_msgType(Comms::server_request::MsgType::AUTH_REQUEST);

    match send_proto_to_client(&mut stream, &auth_request).await {
        Ok(_) => {}
        Err(e) => { println!("Error: {}", e) }
    }


    let login_name = match read_proto_from_client(&mut stream).await {
        Ok(resp) => { resp }
        Err(e) => {
            println!("Error: {}", e);
            return Err(e);
        }
    };

    let username = match login_name.msgType() {
        Comms::server_request::MsgType::AUTH_RESPONSE => { login_name.authResponse.player_name().to_string() }
        _ => {
            println!("Invalid response received.");
            String::new()
        }
    };
    let mut conn = match database_connection.acquire().await {
        Ok(c) => {c}
        Err(e) => {
            println!("Could not get a database connection from the pool.");
            return Err(BidError::DbCannotOpenDatabase);
        }
    };
    let result = match sqlx::query_as::<_, DatabaseUser>("SELECT * from players WHERE username = ?").bind(username.to_owned()).fetch_one(&mut *conn).await {
        Err(e) => {
            println!("Rejected authentication from {}", username);
            let mut auth_reject = ServerRequest::new();
            auth_reject.set_msgType(MsgType::AUTH_REJECT);

            send_proto_to_client(&mut stream, &auth_reject).await?;
            stream.shutdown(Shutdown::Both).ok();
            return Err(BidError::PlayerNotFoundByName);
        },
        Ok(r) => {r},
    };
    println!("Result: {}", result.username);

    let p = Player {
        id: 0,
        name: username,
        wins: 0,
        losses: 0,
        in_game: false,
        stream: stream.clone(),
        player_cooldown: Utc::now()
    };



    let mut ack = ServerRequest::new();
    ack.set_msgType(Comms::server_request::MsgType::ACK);
    send_proto_to_client(&mut stream, &ack).await?;


    match pm_sender.send(Event::NewPlayer {player: p}).await {
        Ok(_) => {}
        Err(e) => {
            println!("Connection loop has lost connection to the player manager with error {:?}", e);
            return Err(BidError::PlayerManagerSendFailed);
        }
    };

    Ok(())
}

async fn accept_loop(addr: impl ToSocketAddrs, database: PathBuf) -> std::result::Result<(), BidError> {

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {l}
        Err(_) => {return Err(BidError::ErrorBindingToInterface)}
    };
    let mut incoming = listener.incoming();

    let pool = SqlitePoolOptions::new().max_connections(10).connect(database.to_str().unwrap()).await.map_err(|_| BidError::DbCannotOpenDatabase)?;

    let (pm_sender, pm_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();

    let pm = task::spawn(player_manager(pm_sender.clone(), pm_receiver, pool.clone()));

    while let Some(stream) = incoming.next().await {
        let stream = match stream {
            Ok(s) => {s}
            Err(_) => {continue}
        };

        println!("Accepting connection from {:?}", stream.peer_addr());
        let _handle = task::spawn(connection_loop(stream.clone(), pm_sender.clone(), pool.clone()));
    }

    Ok(())
}

fn main() -> Result<(), BidError> {
    simple_logger::init_with_level(log::Level::Info).unwrap();
    let args = Args::parse();
    println!("Args: {:?}", args);

    let (pool_broker_sender, pool_broker_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();
    let connection_string = format!("{}:{}", args.interface, args.port);
    let fut = accept_loop(connection_string, args.database);

    task::block_on(fut)?;

    Ok(())
}
