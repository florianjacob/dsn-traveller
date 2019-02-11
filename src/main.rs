#![feature(try_from)]
// enable the await! macro, async support, and the new std::Futures api.
#![feature(await_macro, async_await, futures_api)]

use futures::Future as OldFuture;
use std::convert::TryFrom;
use std::fs;
use std::future::Future;
use std::io;
use std::io::prelude::*;
use std::iter::FromIterator;

use clap::{crate_authors, crate_version, App, Arg, SubCommand};

use ruma_client::{Client, Session};
use ruma_identifiers::{RoomAliasId, RoomId, RoomIdOrAliasId};
use url::Url;

use serde::{Deserialize, Serialize};

use hyper::client::HttpConnector;
use hyper_tls::HttpsConnector;

// Source: https://jsdw.me/posts/rust-asyncawait-preview/
// converts from an old style Future to a new style one:
fn forward<I, E>(
    f: impl OldFuture<Item = I, Error = E> + Unpin,
) -> impl Future<Output = Result<I, E>> {
    use tokio_async_await::compat::forward::IntoAwaitable;
    f.into_awaitable()
}

#[derive(Serialize, Deserialize, Debug)]
struct TravellerConfig {
    #[serde(with = "url_serde")]
    homeserver_url: Url,
    control_room: RoomIdOrAliasId,
}

fn load_config() -> Result<TravellerConfig, io::Error> {
    let file = fs::File::open("config.ron")?;
    let reader = io::BufReader::new(file);
    let config: TravellerConfig =
        ron::de::from_reader(reader).expect("Could not deserialize config.ron");
    Ok(config)
}

fn store_config(config: &TravellerConfig) -> Result<(), io::Error> {
    let file = fs::File::create("config.ron")?;
    let mut buffer = io::BufWriter::new(file);
    write!(
        &mut buffer,
        "{}",
        ron::ser::to_string_pretty(&config, ron::ser::PrettyConfig::default()).unwrap()
    )
}

fn load_session() -> Result<Session, io::Error> {
    let file = fs::File::open("session.ron")?;
    let reader = io::BufReader::new(file);
    let session =
        ron::de::from_reader(reader).expect("could not deserialize session.ron");
    Ok(session)
}

fn store_session(session: Session) -> Result<(), io::Error> {
    let file = fs::File::create("session.ron")?;
    let mut buffer = io::BufWriter::new(file);
    write!(
        &mut buffer,
        "{}",
        ron::ser::to_string_pretty(&session, ron::ser::PrettyConfig::default()).unwrap()
    )
}

fn get_config() -> TravellerConfig {
    match load_config() {
        Ok(config) => config,
        Err(_) => {
            print!("homeserver url: ");
            io::stdout().flush().unwrap();
            let mut homeserver_url = String::new();
            io::stdin().read_line(&mut homeserver_url).unwrap();
            let homeserver_url = Url::parse(homeserver_url.trim()).unwrap();

            print!("control room: ");
            io::stdout().flush().unwrap();
            let mut control_room = String::new();
            io::stdin().read_line(&mut control_room).unwrap();
            let control_room = RoomIdOrAliasId::try_from(control_room.trim()).unwrap();

            let config = TravellerConfig {
                homeserver_url,
                control_room,
            };
            store_config(&config).unwrap();
            config
        },
    }
}

async fn get_client(
    config: &TravellerConfig,
) -> Result<Client<HttpsConnector<HttpConnector>>, ruma_client::Error> {
    let mut needs_login = false;

    let client = match load_session() {
        Ok(session) => Client::https(config.homeserver_url.clone(), Some(session)).unwrap(),
        Err(_) => {
            needs_login = true;
            Client::https(config.homeserver_url.clone(), None).unwrap()
        },
    };

    if needs_login {
        print!("username: ");
        io::stdout().flush().unwrap();
        let mut username = String::new();
        io::stdin().read_line(&mut username).unwrap();
        let username = String::from(username.trim());

        print!("password: ");
        io::stdout().flush().unwrap();
        let mut password = String::new();
        io::stdin().read_line(&mut password).unwrap();
        let password = String::from(password.trim());

        let device_id = format!(
            "rust-dsn-traveller on {}",
            hostname::get_hostname().unwrap()
        );

        let session = await!(forward(client.log_in(username, password, Some(device_id)))).unwrap();
        store_session(session).unwrap();
        eprintln!("Logged in.");
    }
    Ok(client)
}

async fn join(room_list: Vec<String>) -> Result<(), ruma_client::Error> {
    let config = get_config();
    let client = await!(get_client(&config))?;

    let room_aliases = Vec::from_iter(room_list.into_iter().map(|room| {
        RoomAliasId::try_from(&room[..]).unwrap_or_else(|_| panic!("invalid room alias: {}", room))
    }));

    let (join_count, invite_count, leave_count) =
        await!(dsn_traveller::join_rooms(client.clone(), room_aliases))?;
    eprintln!("finished joining rooms");

    let message = format!("Good evening, Gentlemen! \
        Today I learned about {} new rooms, was invited to {} new rooms, and I'm not a member of {} rooms.",
        join_count, invite_count, leave_count);

    let control_room_id = await!(dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    ))
    .expect("Could not resolve control room alias");

    await!(dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ))?;
    eprintln!("{}", message);

    Ok(())
}

async fn crawl() -> Result<(), ruma_client::Error> {
    let config = get_config();
    let client = await!(get_client(&config))?;

    let (room_count, user_count, server_count) = await!(dsn_traveller::crawl(client.clone()))?;
    eprintln!("queried room membership");

    let message = format!(
        "Good evening, Gentlemen! \
         On my travelling, I visited {} rooms on {} different servers, and saw {} people!",
        room_count, server_count, user_count,
    );

    let control_room_id = await!(dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    ))
    .expect("Could not resolve control room alias");

    await!(dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ))?;
    eprintln!("{}", message);

    Ok(())
}

async fn exit_all() -> Result<(), ruma_client::Error> {
    let config = get_config();

    let client = await!(get_client(&config))?;

    let control_room_id = await!(dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    ))
    .expect("Could not resolve control room alias");

    let (left_count, joined_count) = await!(dsn_traveller::exit_all(
        client.clone(),
        control_room_id.clone()
    ))?;

    let message = format!(
        "Good bye, Gentlemen! \
         Today, I departed from {} of the {} rooms I visited.",
        left_count, joined_count
    );

    await!(dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ))?;
    eprintln!("{}", message);

    Ok(())
}

async fn exit(room_id: RoomId) -> Result<(), ruma_client::Error> {
    let config = get_config();
    let client = await!(get_client(&config))?;

    let control_room_id = await!(dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    ))
    .expect("Could not resolve control room alias");

    let message = match await!(dsn_traveller::exit(client.clone(), room_id.clone())) {
        Ok(_) => format!(
            "Good bye, Gentlemen! Today, I successfully departed from room {}.",
            room_id
        ),
        Err(e) => format!(
            "Gentlemen, there was a hitch with leaving from room {}! {:?}",
            room_id, e
        ),
    };

    await!(dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ))?;
    eprintln!("{}", message);

    Ok(())
}

fn main() {
    let matches = App::new("DSN Traveller")
        .version(crate_version!())
        .author(crate_authors!())
        .about("Travelling the Matrix network, for Science!")
        .subcommand(SubCommand::with_name("join")
                    .about("join the given rooms")
                    .display_order(1)
                    .arg(Arg::with_name("stdin")
                         .help("read room aliases from stdin instead of positional arguments, one alias per line")
                         .long("stdin")
                         .conflicts_with("rooms"))
                    .arg(Arg::with_name("room_aliases")
                         .help("room aliases to join")
                         .conflicts_with("stdin")
                         .multiple(true))
                   )
        .subcommand(SubCommand::with_name("crawl")
                    .display_order(2)
                    .about("visit all joined rooms and store the network graph")
                   )
        .subcommand(SubCommand::with_name("exit")
                    .display_order(3)
                    .about("leave and forget given room id, or all previously-joined rooms if no id is given")
                    .arg(Arg::with_name("room_id")
                         .help("room id to leave & forget"))
                   )
        .get_matches();

    let future = async move {
        match matches.subcommand() {
            // ("join", Some(_)) => {
            ("join", Some(join_matches)) => {
                let room_list: Vec<String> = {
                    if join_matches.is_present("stdin") {
                        let stdin = io::stdin();
                        let lines = stdin.lock().lines().map(|line| line.unwrap());
                        Vec::from_iter(lines)
                    } else {
                        match join_matches.values_of("room_aliases") {
                            Some(aliases) => Vec::from_iter(aliases.map(|s| s.to_string())),
                            None => Vec::new(),
                        }
                    }
                };

                await!(join(room_list)).unwrap();
            },
            ("crawl", Some(_)) => await!(crawl()).unwrap(),
            ("exit", Some(exit_matches)) => {
                let room_id = {
                    if exit_matches.is_present("room_id") {
                        let room_id = exit_matches.value_of("room_id").unwrap();
                        let room_id =
                            RoomId::try_from(room_id).expect("Unable to parse given RoomId");
                        Some(room_id)
                    } else {
                        None
                    }
                };
                if let Some(room_id) = room_id {
                    await!(exit(room_id)).unwrap();
                } else {
                    await!(exit_all()).unwrap();
                }
            },
            ("", None) => {
                eprintln!("No subcommand given.");
            },
            _ => unreachable!(),
        }
    };

    tokio::run_async(future);
}
