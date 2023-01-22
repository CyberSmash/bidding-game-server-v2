use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
    future::TimeoutError,
};

use std::{thread, time};

use futures::{stream::FuturesUnordered, StreamExt};

use std::fmt;
use futures::channel::mpsc;
use futures::sink::SinkExt;
use std::{
    collections::hash_map::{HashMap, Entry},
    sync::Arc,
};
use std::io::{BufWriter, ErrorKind};
use std::num::ParseIntError;
//use std::net::TcpStream;
use std::time::Duration;
use crate::mpsc::SendError;

type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;


const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(500);
const CLIENT_ERROR_MAX: u32 = 3;
const MAX_GAME_ROUNDS: u32 = 10;
const BOTTLE_MIN: i32 = -5;
const BOTTLE_MAX: i32 = 5;

enum GameStatus {
    Abandoned,
    Completed,
    Draw,
    Disconnect,
}

enum Error {

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
    Good
    {
        id: u32,
    },
    Bad
}

struct GameMasterInfo {
    id: u32,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
}

struct GameResult
{
    id: u32,
    winner: String,
    loser: String,
    status: GameStatus,
}


// The player managers representation of a player. This is the authoritative
// copy of the player information.
struct Player {
    name: String,
    wins: u64,
    losses: u64,
    in_game: bool,
    stream: TcpStream,
}

// Only the info that the Game Master needs of a player.
struct GMPlayer {
    name: String,
    stream: TcpStream,
}

fn get_player_info_for_game(players: &mut HashMap<String, Player>, player_name: &String) -> Option<GMPlayer> {
    return match players.entry(player_name.clone())
    {
        Entry::Occupied(mut ent) => {
            let p = ent.get_mut();
            p.in_game = true;
            Some(GMPlayer { name: p.name.clone(), stream: p.stream.clone() })
        }
        Entry::Vacant(_) => { None }
    };
}

async fn read_from_client(stream: &TcpStream) -> std::io::Result<String> {
    let mut reader = BufReader::new(stream.clone());
    let mut lines = reader.lines();
    let response = match lines.next().await {
        None => {
            println!("Couldn't get data from player");
            return Ok(String::new());
        }
        Some(r) => { match r {
            Ok(resp) => {resp}
            Err(err) => {
                println!("An unknown error occured reading lines. {}", err);
                return Ok(String::new());
            }
        }}
    };
    //Some(response)
    Ok(response)
}

/**
 * Send an arbitrary String message to the client.
 * TODO: Fix up some of the error handling so the calling thread knows there was a
 * problem.
 */
async fn send_to_client(msg: &str, stream: &mut TcpStream)
{
    match stream.write_all(msg.as_bytes()).await {
        Ok(_) => {}
        Err(_) => {println!("Error: I was unable to contact the client. Have they gone away?")}
    }

}

async fn get_bid_from_client(stream: &mut TcpStream, player_money_left: u32) -> Option<u32> {
    let mut error_count = 0;
    while error_count < 3 {
        send_to_client("bid\n", stream).await;
        let client_response = match async_std::io::timeout(SOCKET_READ_TIMEOUT, read_from_client(stream)).await {
            Ok(response_str) => { response_str }
            Err(_) => {
                error_count += 1;
                send_to_client("timeout\n", stream).await;
                continue;
            }
        };

         let bid_num = match client_response.parse::<u32>() {
            Ok(bid) => {
                bid
            }
            Err(_) => {
                error_count += 1;
                send_to_client("badbid\n", stream).await;
                continue;
            }
         };
        if bid_num > player_money_left {
            error_count += 1;
            send_to_client("badbid\n", stream).await;
            continue;
        }
        else {
            send_to_client("ok\n", stream).await;
            return Some(bid_num);
        }
    }
    None
}


async fn run_game(id: u32, stream_a: &mut TcpStream, name_a: &String,
                  stream_b: &mut TcpStream, name_b: &String) -> GameResult {
    let mut player_a_money_left = 100;
    let mut player_b_money_left = 100;
    let mut bottle_position: i32 = 0;

    let gr_a_abandoned = GameResult {
        id: id,
        winner: name_b.clone(),
        loser: name_a.clone(),
        status: GameStatus::Abandoned
    };
    let gr_b_abandoned = GameResult {
        id: id,
        winner: name_a.clone(),
        loser: name_b.clone(),
        status: GameStatus::Abandoned
    };
    let mut gr = GameResult {
        id: id,
        winner: "".to_string(),
        loser: "".to_string(),
        status: GameStatus::Abandoned
    };
    let start_msg = format!("start {}/{} {}/{}\n", name_a, player_a_money_left, name_b, player_b_money_left);
    stream_a.write_all(start_msg.as_bytes()).await;
    stream_b.write_all(start_msg.as_bytes()).await;

    for round in 0..MAX_GAME_ROUNDS {

        if bottle_position == BOTTLE_MIN || bottle_position == BOTTLE_MAX {
            gr.status = GameStatus::Completed;
            if bottle_position == BOTTLE_MIN {
                gr.winner = name_b.clone();
                gr.loser = name_a.clone();
            }
            else {
                gr.winner = name_a.clone();
                gr.loser = name_b.clone();
            }

            return gr;
        }
        let pos_msg = format!("pos {}\n", bottle_position);

        match stream_a.write_all(pos_msg.as_bytes()).await {
            Ok(_) => {}
            Err(_) => {
                return gr_a_abandoned;
            }
        }
        match stream_b.write_all(pos_msg.as_bytes()).await {
            Ok(_) => {}
            Err(_) => {
                return gr_b_abandoned;
            }
        }


        let bid_a = match get_bid_from_client(stream_a,
                                              player_a_money_left).await {
            None => {
                println!("Could not get bid from {}. Abandon game.", name_a);
                return gr_a_abandoned;
            }
            Some(bid) => {
                bid
            }
        };

        let bid_b = match get_bid_from_client(stream_b,
                                              player_b_money_left).await
        {
            None => {
                println!("Could not get bid from {}. Abandon game.", name_b);
                return gr_b_abandoned;
            }
            Some(bid) => {
                bid
            }
        };

        let bid_result = format!("result {}/{}\n", bid_a, bid_b);

        if bid_a > bid_b {
            player_a_money_left -= bid_a;
            bottle_position += 1;

        }
        if bid_a < bid_b {
            player_b_money_left -= bid_b;
            bottle_position -= 1;
        }

        match stream_a.write_all(bid_result.as_bytes()).await {
            Ok(_) => {}
            Err(_) => {
                return gr_a_abandoned;
            }
        }
        match stream_b.write_all(bid_result.as_bytes()).await {
            Ok(_) => {}
            Err(_) => {
                return gr_b_abandoned;
            }
        }


    } // end for-loop


    gr
}

async fn game_master(mut pm_sender: Sender<Event>, mut gm_receiver: Receiver<Event>, id: u32)
{


    let gm_backoff = Duration::from_millis(500);
    println!("Game Master: {} is alive!", id);

    pm_sender.send(Event::NeedPlayers {id}).await;
    while let Some(event) = gm_receiver.next().await {
        match event {
            Event::NewPlayer { .. } => {}
            Event::NewGame { name_a, name_b, mut stream_a, mut stream_b } => {
                let game_result = run_game(id, &mut stream_a, &name_a, &mut stream_b, &name_b).await;
                match game_result.status {
                    GameStatus::Abandoned => {
                        println!("It looks like player {} disconnected before we could finish the game.", game_result.loser);
                    }
                    GameStatus::Completed => {
                        let winner_message = format!("final win {}", game_result.winner);
                        stream_a.write_all(winner_message.as_bytes()).await;
                        stream_b.write_all(winner_message.as_bytes()).await;

                    }
                    GameStatus::Draw => {
                        let draw_msg = "final draw";
                        stream_a.write_all(draw_msg.as_bytes()).await;
                        stream_b.write_all(draw_msg.as_bytes()).await;

                    }
                    GameStatus::Disconnect => {
                        println!("It looks like player {} disconnected.", game_result.loser);
                    }
                }
                pm_sender.send(Event::GameOver { result: game_result }).await;

            }
            Event::Good {id: i32} => {}
            Event::Bad => {
                thread::sleep(gm_backoff);
                pm_sender.send(Event::NeedPlayers {id}).await;
            }
            _ => {}
        }
    }
}


fn get_free_players(players: &HashMap<String, Player>) -> Vec<String>
{
    let mut indexes: Vec<String> = vec!();
    for (name, player) in players.into_iter() {
        if !player.in_game {
            indexes.push(name.clone());
        }
    }
    return indexes;
}

async fn handle_need_players(players: &mut HashMap<String, Player>, gm_sender: &mut Sender<Event>) -> bool {
    let free_players = get_free_players(&players);

    if free_players.len() < 2 {
        match gm_sender.send(Event::Bad).await
        {
            Ok(_) => {}
            Err(e) => {println!("Error sending message to game master.");}
        }
        return false;
    }
    let player_a = match get_player_info_for_game(players, &free_players[0])
    {
        None => {
            println!("I errored out trying to find a player. The player {} was suddenly not found.", free_players[0]);
            gm_sender.send(Event::Bad).await;
            return false;
        }
        Some(p) => {p}
    };

    let player_b = match get_player_info_for_game(players, &free_players[1])
    {
        None => {
            println!("I errored out trying to find a player. The player {} was suddenly not found.", free_players[1]);
            gm_sender.send(Event::Bad).await;
            return false;
        }
        Some(p) => {p}
    };

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

async fn player_manager(mut sender: Sender<Event>, mut receiver: Receiver<Event>) {
    let mut players: HashMap<String, Player> = HashMap::new();
    let mut game_comms: HashMap<u32, Sender<Event>> = HashMap::new();
    //let  mut game_masters = FuturesUnordered::new();
    let max_games = 4;

    for id in 0..max_games {
        let (gm_sender, gm_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();
        game_comms.insert(id.clone(), gm_sender);
        task::spawn(game_master(sender.clone(), gm_receiver, id));
    }


    while let Some(event) = receiver.next().await {
        match event {
            Event::NewPlayer { mut player } => {
                println!("The player manager has recvd. {}", player.name);
                players.insert(player.name.clone(), player);
                println!("Num players: {}", players.len())
            }
            Event::GameOver {
                result
            } => {
                println!("Game {} has finished, with {} winner and {} loser", result.id, result.winner, result.loser );
                if let Some(peer) = players.get_mut(&result.winner) {
                    peer.in_game = false;
                    peer.wins += 1;
                }
                if let Some(peer) = players.get_mut(&result.loser)
                {
                    peer.in_game = false;
                    peer.losses += 1;
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

            Event::Good {id} => { println!("Received Good from {}", id)}
            Event::Bad => {}
            _ => {}
        }
    }

}

async fn connection_loop(mut stream: TcpStream, mut pm_sender: Sender<Event>) -> Result<()>
{
    let reader = BufReader::new(stream.clone());
    //let  writer = BufWriter::new(&stream);
    let mut lines = reader.lines();
    let login_msg = "login\n";
    send_to_client(login_msg, &mut stream).await;

    let name = match lines.next().await {
        None => Err("Peer disconnected.")?,
        Some(line) => line?,
    };

    send_to_client("ok\n", &mut stream).await;
    //stream.write_all(name.as_bytes()).await;

    let p = Player {
        name,
        wins: 0,
        losses: 0,
        in_game: false,
        stream
    };
    pm_sender.send(Event::NewPlayer {player: p}).await?;


    Ok(())
}

async fn accept_loop(addr: impl ToSocketAddrs) -> Result<()> {

    let listener = TcpListener::bind(addr).await?;
    let mut incoming = listener.incoming();

    let (pm_sender, pm_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();

    let pm = task::spawn(player_manager(pm_sender.clone(), pm_receiver));

    while let Some(stream) = incoming.next().await {
        let stream = stream?;

        println!("Accepting connection from {}", stream.peer_addr()?);
        let _handle = task::spawn(connection_loop(stream.clone(), pm_sender.clone()));
    }

    Ok(())
}


fn sender_func(mut sender: Sender<Event>)
{
    sender.send(Event::Bad);

}

fn recv_func(mut recvr: Receiver<Event>)
{
    println!("In the func!");
}

fn main() -> Result<()> {

    let pool_broker_sender: Sender<Event>;
    let pool_broker_receiver: Receiver<Event>;
    let (pool_broker_sender, pool_broker_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();
    //let () = mpsc::unbounded();
    sender_func(pool_broker_sender);
    //sender_func(pool_broker_receiver);
    recv_func(pool_broker_receiver);
    println!("Hello, world!");
    let fut = accept_loop("127.0.0.1:8080");
    task::block_on(fut);

    Ok(())
}
