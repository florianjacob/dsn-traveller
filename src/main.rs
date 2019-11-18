use std::convert::TryFrom;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::iter::FromIterator;

use clap::{crate_authors, crate_version, App, Arg, SubCommand};

use ruma_client::{
    HttpsClient, Session,
    identifiers::{RoomAliasId, RoomId, RoomIdOrAliasId},
};
use url::Url;

use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Debug)]
struct TravellerConfig {
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
) -> Result<HttpsClient, ruma_client::Error> {
    let mut needs_login = false;

    let client = match load_session() {
        Ok(session) => HttpsClient::https(config.homeserver_url.clone(), Some(session)).unwrap(),
        Err(_) => {
            needs_login = true;
            HttpsClient::https(config.homeserver_url.clone(), None).unwrap()
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

        let session = client.log_in(username, password, Some(device_id)).await.unwrap();
        store_session(session).unwrap();
        eprintln!("Logged in.");
    }
    Ok(client)
}

async fn join(room_list: Vec<String>) -> Result<(), ruma_client::Error> {
    let config = get_config();
    let client = get_client(&config).await?;

    let room_aliases = Vec::from_iter(room_list.into_iter().map(|room| {
        RoomAliasId::try_from(&room[..]).unwrap_or_else(|_| panic!("invalid room alias: {}", room))
    }));

    let (join_count, invite_count, leave_count) =
        dsn_traveller::join_rooms(client.clone(), room_aliases).await?;
    eprintln!("finished joining rooms");

    let message = format!("Good evening, Gentlemen! \
        Today I learned about {} new rooms, was invited to {} new rooms, and I'm not a member of {} rooms.",
        join_count, invite_count, leave_count);

    let control_room_id = dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    )
    .await.expect("Could not resolve control room alias");

    dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ).await?;
    eprintln!("{}", message);

    Ok(())
}

async fn crawl() -> Result<(), ruma_client::Error> {
    let config = get_config();
    let client = get_client(&config).await?;

    let (room_count, user_count, server_count) = dsn_traveller::crawl(client.clone()).await?;
    eprintln!("queried room membership");

    let message = format!(
        "Good evening, Gentlemen! \
         On my travelling, I visited {} rooms on {} different servers, and saw {} people!",
        room_count, server_count, user_count,
    );

    let control_room_id = dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    )
    .await .expect("Could not resolve control room alias");

    dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ).await?;
    eprintln!("{}", message);

    Ok(())
}

async fn exit_all() -> Result<(), ruma_client::Error> {
    let config = get_config();

    let client = get_client(&config).await?;

    let control_room_id = dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    )
    .await.expect("Could not resolve control room alias");

    let (left_count, joined_count) = dsn_traveller::exit_all(
        client.clone(),
        control_room_id.clone()
    ).await?;

    let message = format!(
        "Good bye, Gentlemen! \
         Today, I departed from {} of the {} rooms I visited.",
        left_count, joined_count
    );

    dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ).await?;
    eprintln!("{}", message);

    Ok(())
}

async fn exit(room_id: RoomId) -> Result<(), ruma_client::Error> {
    let config = get_config();
    let client = get_client(&config).await?;

    let control_room_id = dsn_traveller::into_room_id(
        client.clone(),
        config.control_room.clone()
    )
    .await.expect("Could not resolve control room alias");

    let message = match dsn_traveller::exit(client.clone(), room_id.clone()).await {
        Ok(_) => format!(
            "Good bye, Gentlemen! Today, I successfully departed from room {}.",
            room_id
        ),
        Err(e) => format!(
            "Gentlemen, there was a hitch with leaving from room {}! {:?}",
            room_id, e
        ),
    };

    dsn_traveller::send_message(
        client.clone(),
        control_room_id,
        message.clone()
    ).await?;
    eprintln!("{}", message);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), ruma_client::Error> {
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

            join(room_list).await
        },
        ("crawl", Some(_)) => crawl().await,
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
                exit(room_id).await
            } else {
                exit_all().await
            }
        },
        ("", None) => {
            eprintln!("No subcommand given.");
            // TODO: this could be done cleaner with a custom Error type
            std::process::exit(1)
        },
        _ => unreachable!(),
    }
}
