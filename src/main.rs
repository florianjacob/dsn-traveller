#![feature(generators)]
#![feature(proc_macro, proc_macro_non_items)]
#![feature(try_from)]

extern crate futures_await as futures;
extern crate tokio_core;
extern crate ruma_client;
extern crate ruma_identifiers;
extern crate url;
extern crate hostname;
extern crate hyper;
extern crate hyper_tls;

#[macro_use]
extern crate clap;

extern crate dsn_traveller;

#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate url_serde;
extern crate ron;


use std::fs;
use std::io;
use std::io::prelude::*;
use std::convert::TryFrom;
use std::iter::FromIterator;

use clap::{Arg, App, SubCommand};

use url::Url;
use futures::prelude::*;
use ruma_client::{Client, Session};
use ruma_identifiers::{RoomAliasId, RoomIdOrAliasId, UserId};
use tokio_core::reactor::{Core, Handle};


use hyper_tls::HttpsConnector;
use hyper::client::HttpConnector;



#[derive(Serialize, Deserialize, Debug)]
struct TravellerConfig {
    #[serde(with = "url_serde")]
    homeserver_url: Url,
    control_room: RoomIdOrAliasId,
}

// copy of ruma_client::Session for deriving debug & deserialize
// alternative would be https://serde.rs/remote-derive.html, but that looks much more cumbersome,
// especially as ruma_client::Session will probably implement Serialize, Deserialize itself
// eventually.
#[derive(Serialize, Deserialize, Debug)]
struct StoredSession {
    access_token: String,
    user_id: UserId,
    device_id: String,
}
impl From<Session> for StoredSession {
    fn from(session: Session) -> Self {
        StoredSession {
            access_token: session.access_token().into(),
            user_id: session.user_id().clone(),
            device_id: session.device_id().into(),
        }
    }
}

impl Into<Session> for StoredSession {
    fn into(self) -> Session {
        Session::new(self.access_token, self.user_id, self.device_id)
    }
}


fn load_config() -> Result<TravellerConfig, io::Error> {
    let file = fs::File::open("config.ron")?;
    let reader = io::BufReader::new(file);
    let config: TravellerConfig = ron::de::from_reader(reader).expect("Could not deserialize config.ron");
    Ok(config)
}

fn store_config(config: &TravellerConfig) -> Result<(), io::Error> {
    let file = fs::File::create("config.ron")?;
    let mut buffer = io::BufWriter::new(file);
    write!(&mut buffer, "{}", ron::ser::to_string_pretty(&config, ron::ser::PrettyConfig::default()).unwrap())
}

fn load_session() -> Result<Session, io::Error> {
    let file = fs::File::open("session.ron")?;
    let reader = io::BufReader::new(file);
    let session: StoredSession = ron::de::from_reader(reader).expect("could not deserialize session.ron");
    Ok(session.into())
}

fn store_session(session: Session) -> Result<(), io::Error> {
    let file = fs::File::create("session.ron")?;
    let mut buffer = io::BufWriter::new(file);
    let session: StoredSession = session.into();
    write!(&mut buffer, "{}", ron::ser::to_string_pretty(&session, ron::ser::PrettyConfig::default()).unwrap())
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

fn get_client(tokio_handle: &Handle, config: &TravellerConfig) -> impl Future<Item = Client<HttpsConnector<HttpConnector>>, Error = ruma_client::Error> {
    let mut needs_login = false;

    let client = match load_session() {
        Ok(session) => {
            Client::https(tokio_handle, config.homeserver_url.clone(), Some(session)).unwrap()
        },
        Err(_) => {
            needs_login = true;
            Client::https(tokio_handle, config.homeserver_url.clone(), None).unwrap()
        },
    };

    async_block! {
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

            let device_id = format!("rust-dsn-traveller on {}", hostname::get_hostname().unwrap());

            await!(client.log_in(username, password, Some(device_id))).unwrap();
            store_session(client.session()).unwrap();
            eprintln!("Logged in.");
        }
        Ok(client)
    }
}

fn join(
    tokio_handle: Handle,
    room_list: Vec<String>,
    ) -> impl Future<Item = (), Error = ruma_client::Error> + 'static {

    let config = get_config();

    async_block! {
        let client = await!(get_client(&tokio_handle, &config))?;

        let room_aliases = Vec::from_iter(room_list.into_iter()
                                          .map(|room| RoomAliasId::try_from(&room[..]).expect(&format!("invalid room alias: {}", room)))
                                         );

        let (join_count, invite_count, leave_count) = await!(dsn_traveller::join_rooms(client.clone(), room_aliases))?;
        eprintln!("finished joining rooms");

        let message = format!("Good evening, Gentlemen! \
            Today I learned about {} new rooms, was invited to {} new rooms, and I'm not a member of {} rooms.",
            join_count, invite_count, leave_count);

        let control_room_id = await!(dsn_traveller::into_room_id(client.clone(), config.control_room.clone())).expect("Could not resolve control room alias");

        await!(dsn_traveller::send_message(client.clone(), control_room_id, message.clone()))?;
        eprintln!("{}", message);

        Ok(())
    }
}

fn crawl(
    tokio_handle: Handle,
    ) -> impl Future<Item = (), Error = ruma_client::Error> + 'static {

    let config = get_config();

    async_block! {
        let client = await!(get_client(&tokio_handle, &config))?;

        let (room_count, user_count, server_count) = await!(dsn_traveller::crawl(client.clone()))?;
        eprintln!("queried room membership");

        let message = format!("Good evening, Gentlemen! \
            On my travelling, I visited {} rooms on {} different servers, and saw {} people!",
            room_count, server_count, user_count,);

        let control_room_id = await!(dsn_traveller::into_room_id(client.clone(), config.control_room.clone())).expect("Could not resolve control room alias");

        await!(dsn_traveller::send_message(client.clone(), control_room_id, message.clone()))?;
        eprintln!("{}", message);

        Ok(())
    }
}

fn leave(
    tokio_handle: Handle,
    ) -> impl Future<Item = (), Error = ruma_client::Error> + 'static {

    let config = get_config();

    async_block! {
        let client = await!(get_client(&tokio_handle, &config))?;

        let control_room_id = await!(dsn_traveller::into_room_id(client.clone(), config.control_room.clone())).expect("Could not resolve control room alias");

        let (left_count, joined_count) = await!(dsn_traveller::leave(client.clone(), control_room_id.clone()))?;

        let message = format!("Good bye, Gentlemen! \
            Today, I departed from {} of the {} rooms I visited.",
            left_count, joined_count);

        await!(dsn_traveller::send_message(client.clone(), control_room_id, message.clone()))?;
        eprintln!("{}", message);

        Ok(())
    }
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
        .subcommand(SubCommand::with_name("leave")
                    .display_order(3)
                    .about("leave all previously-joined rooms")
                   )
        .get_matches();

    let mut core = Core::new().unwrap();
    let handle = core.handle().clone();

    let future = async_block! {
        match matches.subcommand() {
            ("join", Some(_)) => {
                let room_list: Vec<String> = {
                    // would need non-lexical lifetimes to use the ("join", Some(join_matches))
                    // as that borrow will not end before the await, running in a
                    // "borrow may still be in use when generator yields"
                    let join_matches = matches.subcommand_matches("join").unwrap();
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

                await!(join(handle, room_list))
            },
            ("crawl", Some(_)) => await!(crawl(handle)),
            ("leave", Some(_)) => await!(leave(handle)),
            ("", None) => {
                eprintln!("No subcommand given.");
                Ok(())
            },
            _ => unreachable!(),
        }
    };

    core.run(future).unwrap();
}
