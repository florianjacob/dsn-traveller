#![feature(generators)]
#![feature(proc_macro, proc_macro_non_items)]
#![feature(try_from)]

extern crate futures_await as futures;
extern crate futures_timer;
extern crate ruma_client;
extern crate ruma_events;
extern crate ruma_identifiers;
extern crate tokio_core;
extern crate url;
extern crate hyper;
extern crate petgraph;
extern crate petgraph_graphml;
extern crate rand;

extern crate serde;
extern crate serde_json;
extern crate ron;
extern crate regex;

#[macro_use]
extern crate lazy_static;

extern crate matrixgraph;


use std::convert::TryFrom;
use std::iter::FromIterator;
use std::collections::{HashSet, HashMap};
use std::time;

use futures::prelude::*;
use ruma_client::Client;
use ruma_client::api::r0;
use ruma_events::stripped::StrippedState;
use ruma_events::EventType;
use ruma_events::room::message::{MessageEventContent, MessageType, TextMessageEventContent};
use ruma_events::room::member::MembershipState;
use ruma_identifiers::{RoomId, RoomAliasId, RoomIdOrAliasId, EventId, UserId};
use futures_timer::Delay;

use petgraph::Graph;

use hyper::client::Connect;

use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::collections::hash_map::DefaultHasher;
use std::collections::hash_map::RandomState;
use std::hash::{Hash, Hasher, BuildHasher};
use rand::Rng;


use matrixgraph::{Node, NodeType};

lazy_static! {
    static ref TXN_ID: AtomicUsize = {
        let mut txn_id = ATOMIC_USIZE_INIT;
        randomize_txnid(&mut txn_id);
        txn_id
    };
}

// try best to avoid rate limiting for federation requests for resolve_alias and join_room
// 2500 rooms * 2s = 1.5 Days
// 2500 rooms * 5s = 3 Days
// more info on rate limiting:
// https://github.com/matrix-org/synapse/blob/9bba6ebaa903a81cd94fada114aa71e20b685adb/synapse/config/ratelimiting.py#L30
// in case of room_crawl, it's only my own home server rate limiting,
// as this does not require federation requests, I should be able to raise that limit arbitrarily
// 2500 rooms * 0.2 = 8 minutes
// 5 seconds resulted in load factor of 4, spacing out to have more time for the computation
static ROOM_JOIN_DELAY: time::Duration = time::Duration::from_millis(64000);
static ROOM_CRAWL_DELAY: time::Duration = time::Duration::from_millis(500);

// if we continue to use the same access token,
// we need to try to have unique txnids.
// alternatively, we could store the last txnid on shutdown
fn randomize_txnid(txn_id: &mut AtomicUsize) {
    let mut rng = rand::thread_rng();
    let id = rng.gen::<usize>();
    txn_id.store(id, Ordering::Relaxed);
}


#[async]
pub fn send_message<C: Connect>(client: Client<C>, room_id: RoomId, message: String) -> Result<EventId, ruma_client::Error> {
    use r0::send::send_message_event;
    let response = await!(send_message_event::call(
            client.clone(),
            send_message_event::Request {
                room_id: room_id,
                event_type: EventType::RoomMessage,
                txn_id: TXN_ID.fetch_add(1, Ordering::Relaxed).to_string(),
                data: MessageEventContent::Text(TextMessageEventContent {
                    body: message,
                    msgtype: MessageType::Text,
                }),
            }
            ))?;
    Ok(response.event_id)
}

#[async]
fn joined_rooms<C: Connect>(client: Client<C>) -> Result<Vec<RoomId>, ruma_client::Error> {
    use r0::membership::joined_rooms;
    let response = await!(joined_rooms::call(
            client.clone(),
            joined_rooms::Request {}
            ))?;
    Ok(response.joined_rooms)
}

#[async]
fn sync_rooms<C: Connect>(client: Client<C>) -> Result<r0::sync::sync_events::Rooms, ruma_client::Error> {
    use r0::filter;
    let filter_all = filter::Filter {
        not_types: vec!["*".to_owned()],
        limit: None,
        senders: Vec::new(),
        types: Vec::new(),
        not_senders: Vec::new(),
    };
    let filter_all_events = filter::RoomEventFilter {
        not_types: vec!["*".to_owned()],
        not_rooms: Vec::new(),
        limit: None,
        rooms: Vec::new(),
        not_senders: Vec::new(),
        senders: Vec::new(),
        types: Vec::new(),
    };
    let filter_room_events = filter::RoomEventFilter {
        not_types: Vec::new(),
        not_rooms: Vec::new(),
        limit: None,
        rooms: Vec::new(),
        not_senders: Vec::new(),
        senders: Vec::new(),
        types: vec!["m.room.canonical_alias".to_owned()],
    };
    let room_filter = filter::RoomFilter {
        include_leave: Some(true),
        account_data: Some(filter_all_events.clone()),
        timeline: Some(filter_all_events.clone()),
        ephemeral: Some(filter_all_events.clone()),
        state: Some(filter_room_events),
        not_rooms: Vec::new(),
        rooms: Vec::new(),
    };
    let filter_definition = filter::FilterDefinition {
        event_fields: Vec::new(),
        event_format: None,
        account_data: Some(filter_all.clone()),
        room: Some(room_filter.clone()),
        presence: Some(filter_all.clone()),
    };
    let filter_string = serde_json::to_string(&filter_definition).expect("filter json serialization failed");

    use r0::sync::sync_events;
    let response = await!(sync_events::call(
            client.clone(),
            sync_events::Request {
                // This does not work, as the serde_urlencoded can't treat the complex struct,
                // in needs to be converted to json first before serde_urlencoded gets it
                // filter: Some(sync_events::Filter::FilterDefinition(filter_definition)),
                // ISSUE: https://github.com/ruma/ruma-api-macros/issues/3
                // HACK: the server tells by the leading '{' whether it's json or not,
                // so we can actually to the conversion ourselves upfront without telling ruma-client
                filter: Some(sync_events::Filter::FilterId(filter_string)),
                since: None,
                full_state: Some(true),
                set_presence: None,
                timeout: None,
            }
            )).expect("Could not get sync response");
    eprintln!("next batch: {}", response.next_batch);
    Ok(response.rooms)
}

#[async]
pub fn join_rooms<C: Connect>(client: Client<C>, room_aliases: Vec<RoomAliasId>) -> Result<(usize, usize, usize), ruma_client::Error> {
    eprintln!("Syncing…");
    let rooms = await!(sync_rooms(client.clone())).expect("error syncing");
    eprintln!("Already joined rooms: {}", rooms.join.len());
    // rooms that the bot was once a member of, but either left it (bot doesn't do that),
    // was kicked or was banned. Rooms stay in here as long as I don't click on "remove" in Riot, it seems.
    // => this is the difference between leave and forget endpoint, it seems.
    // As invites do not check against this, this results in rejoin if kicked, but permission denied error if banned.
    eprintln!("Left rooms (for whatever reason): {:?}", rooms.leave.keys());

    // The simulation will assume the same message sending behaviour for all users.
    // So skip twitter rooms as the users in there also mirror twitter followers,
    // they don't actually take part in matrix but are for display purposes only - they never send messages.
    // This is in contrast to e.g. bridged discord or IRC users, so they're left inside the graph.
    let ignore_pattern = regex::Regex::new(r"^#twitter_#").unwrap();

    let mut join_count: usize = 0;
    let mut invite_count: usize = 0;
    let rooms_to_join = room_aliases.len();
    let invites_to_follow = rooms.invite.len();

    for (room_id, invite) in rooms.invite.clone().into_iter() {
        await!(Delay::new(ROOM_JOIN_DELAY)).unwrap();
        let mut canonical_alias = None;
        for event in invite.clone().invite_state.events {
            if let StrippedState::RoomCanonicalAlias(canonical_alias_event) = event {
                canonical_alias = Some(canonical_alias_event.content.alias.clone().unwrap());
                break;
            }
        }

        if let Some(canonical_alias) = canonical_alias {
            if ignore_pattern.is_match(canonical_alias.alias()) {
                eprintln!("ignoring {:?}", canonical_alias);
                continue;
            }
            use r0::membership::join_room_by_id_or_alias;
            match await!(join_room_by_id_or_alias::call(
                    client.clone(),
                    join_room_by_id_or_alias::Request {
                        room_id_or_alias: RoomIdOrAliasId::RoomAliasId(canonical_alias.clone()),
                        third_party_signed: None,
                    }
                    )) {
                Ok(_) => {
                    invite_count += 1;
                    eprintln!("Followed invite to room: {:?} ({}/{})", canonical_alias.clone(), invite_count, invites_to_follow);
                },
                Err(e) => eprintln!("Error joining invited room {}: {:?}", canonical_alias, e),
            };
        }
        else {
            // this seem to be mostly invites from NickServ bots or similar from IRC bridges
            // -> one can directly follow invites by ID, as the inviting server is already known, it seems!
            // TODO: directly join by id and skip canonical alias stuff from above?
            eprintln!("could resolve canonical alias for invited room {:#?}, trying to join by room id", invite);
            use r0::membership::join_room_by_id_or_alias;
            match await!(join_room_by_id_or_alias::call(
                    client.clone(),
                    join_room_by_id_or_alias::Request {
                        room_id_or_alias: RoomIdOrAliasId::RoomId(room_id.clone()),
                        third_party_signed: None,
                    }
                    )) {
                Ok(_) => {
                    invite_count += 1;
                    eprintln!("Followed invite to room: {:?} ({}/{})", room_id.clone(), invite_count, invites_to_follow);
                },
                Err(e) => eprintln!("Error joining invited room through id{:?}: {:?}", invite, e),
            };
        }
    }

    if room_aliases.len() == 0 {
        eprintln!("no new rooms given to join.");
        return Ok((join_count, invite_count, rooms.leave.len()));
    }

    let joined_rooms_set: HashSet<RoomId> = HashSet::from_iter(rooms.join.keys().cloned());
    let left_rooms_set: HashSet<RoomId> = HashSet::from_iter(rooms.leave.keys().cloned());
    let invited_rooms_set: HashSet<RoomId> = HashSet::from_iter(rooms.invite.keys().cloned());


    for alias in room_aliases {
        if ignore_pattern.is_match(alias.alias()) {
            eprintln!("ignoring {:?}", alias);
            continue;
        }

        let room_id = match await!(resolve_alias(client.clone(), alias.clone())) {
            Ok(room_id) => room_id,
            Err(e) => {
                eprintln!("Could not resolve room {}: {:?}", alias, e);
                continue;
            },
        };
        // if the bot is not yet in that room, and was not invited (which was already handled), and
        // has not left that room, i.e. was kicked from that room, try to join.
        if !joined_rooms_set.contains(&room_id) && !invited_rooms_set.contains(&room_id) && !left_rooms_set.contains(&room_id) {
            use r0::membership::join_room_by_id_or_alias;
            match await!(join_room_by_id_or_alias::call(
                    client.clone(),
                    join_room_by_id_or_alias::Request {
                        room_id_or_alias: RoomIdOrAliasId::RoomAliasId(alias.clone()),
                        third_party_signed: None,
                    }
                    )) {
                Ok(_) => {
                    join_count += 1;
                    eprintln!("Joined room: {:?} ({}/{})", alias, join_count, rooms_to_join);
                },
                Err(e) => eprintln!("Error joining room {}: {:?}", room_id, e),
            };

            await!(Delay::new(ROOM_JOIN_DELAY)).expect("wait failed");
        }
        else {
            eprintln!("already joined, invited or was kicked from room {}.", room_id);
        }
    }
    Ok((join_count, invite_count, rooms.leave.len()))
}

#[async]
fn leave_and_forget_room<C: Connect>(client: Client<C>, room_id: RoomId) -> Result<(), ruma_client::Error> {
    use r0::membership::leave_room;
    await!(leave_room::call(
            client.clone(),
            leave_room::Request {
                room_id: room_id.clone(),
            }
            ))?;

    use r0::membership::forget_room;
    await!(forget_room::call(
            client.clone(),
            forget_room::Request {
                room_id,
            }
            ))?;
    Ok(())
}


#[async]
pub fn resolve_alias<C: Connect>(client: Client<C>, room_alias: RoomAliasId) -> Result<RoomId, ruma_client::Error> {
    use r0::alias::get_alias;
    let response = await!(get_alias::call(
            client.clone(),
            get_alias::Request {
                room_alias,
            }
            ))?;
    Ok(response.room_id)
}

#[async]
pub fn into_room_id<C: Connect>(client: Client<C>, room_id_or_alias_id: RoomIdOrAliasId) -> Result<RoomId, ruma_client::Error> {
    match room_id_or_alias_id {
        RoomIdOrAliasId::RoomId(room_id) => Ok(room_id),
        RoomIdOrAliasId::RoomAliasId(alias) => await!(resolve_alias(client.clone(), alias)),
    }
}


/// delivers the user ids of all users currently joined in the given room
#[async]
fn room_members<C: Connect>(client: Client<C>, room_id: RoomId) -> Result<Vec<String>, ruma_client::Error> {
    use r0::sync::get_member_events;
    let response = await!(get_member_events::call(
            client.clone(),
            get_member_events::Request {
                room_id: room_id.clone(),
            }))?;

    // in the case of join membership events it's probably always the case that sender is the same user
    // the event relates to, but actually, the state key is the field building the relationship to the user.
    let state_keys = response.chunk.into_iter()
        .filter(|event| event.content.membership == MembershipState::Join).map(|event| event.state_key).collect();
    Ok(state_keys)
}

fn hash(builder: &BuildHasher<Hasher = DefaultHasher>, x: &impl Hash) -> u64 {
    let mut hasher = builder.build_hasher();
    x.hash(&mut hasher);
    hasher.finish()
}

#[async]
pub fn crawl<C: Connect>(client: Client<C>) -> Result<(usize, usize, usize), ruma_client::Error> {

    // * ignore ourself and voyager, as we are in all rooms but silent, so we won't send messages in the simulation
    // * weho.st and disroot.org requested to opt out as whole server, this will lead to an
    //   anonymized graph in which those servers and the users on them never existed.
    let member_ignore_pattern = regex::Regex::new(
        r"^(@.*:dsn-traveller.dsn.scc.kit.edu|@voyager:t2bot.io|@.*:weho.st|@.*:disroot.org)$"
        ).unwrap();

    let joined_rooms = await!(joined_rooms(client.clone()))?;
    let mut graph: Graph<Node, (), petgraph::Undirected> = Graph::new_undirected();

    let mut room_indexes = HashMap::<RoomId, petgraph::graph::NodeIndex<petgraph::graph::DefaultIx>>::new();
    let mut user_indexes = HashMap::<UserId, petgraph::graph::NodeIndex<petgraph::graph::DefaultIx>>::new();
    let mut server_indexes = HashMap::<String, petgraph::graph::NodeIndex<petgraph::graph::DefaultIx>>::new();

    // pseudonymization:
    // on each crawl, choose a different random has function
    let hash_key = RandomState::new();
    let mut crawled_rooms = 0;
    let rooms_to_crawl = joined_rooms.len();

    for room in joined_rooms {
        await!(Delay::new(ROOM_CRAWL_DELAY)).expect("wait failed");

        let room_node = graph.add_node(Node { kind: NodeType::Room, id: hash(&hash_key, &room) });
        assert!(!room_indexes.contains_key(&room));
        room_indexes.insert(room.clone(), room_node);

        let members = await!(room_members(client.clone(), room.clone()))?;
        // TODO: should I try to completely ignore rooms with 2 members of which one is myself?
        // -> mainly IRC Bridge ChanServ/NickServ/… rooms
        // what about empty rooms, where only traveller (and possibly voyager, or users of
        // opted-out servers) are in?
        for member in members {
            if member_ignore_pattern.is_match(member.as_str()) {
                continue;
            }
            let (_, server) = member.split_at(member.find(':').unwrap() + 1);
            let server = server.to_string();
            if !server_indexes.contains_key(&server) {
                let server_node = graph.add_node(Node { kind: NodeType::Server, id: hash(&hash_key, &server) });
                server_indexes.insert(server.clone(), server_node);
            }

            let user_id = UserId::try_from(member.as_str()).unwrap();
            if !user_indexes.contains_key(&user_id) {
                let user_node = graph.add_node(Node { kind: NodeType::User, id: hash(&hash_key, &user_id) });
                user_indexes.insert(user_id.clone(), user_node);

                let server_node = server_indexes.get(&server).unwrap();
                graph.add_edge(user_node, *server_node, ());
            }

            let user_node = user_indexes.get(&user_id).unwrap();
            graph.add_edge(*user_node, room_node, ());
            let server_node = server_indexes.get(&server).unwrap();
            graph.update_edge(*server_node, room_node, ());
        }
        crawled_rooms += 1;
        eprintln!("Crawled {}/{} rooms", crawled_rooms, rooms_to_crawl);
    }

    let graph = matrixgraph::anonymize_graph(graph);

    matrixgraph::write_graph(&graph).unwrap();

    matrixgraph::export_graph_to_dot(&graph).unwrap();

    matrixgraph::export_graph_to_graphml(&graph).unwrap();

    Ok((room_indexes.len(), user_indexes.len(), server_indexes.len()))
}

#[async]
pub fn leave<C: Connect>(client: Client<C>, control_room: RoomId) -> Result<(usize, usize), ruma_client::Error> {
    let joined_rooms = await!(joined_rooms(client.clone()))?;

    // ignore control room
    let joined_count = joined_rooms.len() - 1;
    let mut left_count = 0;

    // leaving as well as forgetting so that the server could part the federation for that rooms.
    // Also, if we would not forget leaved rooms, they would appear as rooms where the bot has been
    // kicked from on a later join run.
    // TODO: is leave_and_forget_room enough so that the server can be shut down
    // without being a dead member of the federation?
    for room_id in joined_rooms {
        if room_id != control_room {
            await!(Delay::new(ROOM_CRAWL_DELAY)).expect("wait failed");
            match await!(leave_and_forget_room(client.clone(), room_id.clone())) {
                Ok(_) => {
                    left_count += 1;
                    eprintln!("Left room: {:?} ({}/{})", room_id, left_count, joined_count);
                },
                Err(e) => eprintln!("Error leaving / forgetting room {}: {:?}", room_id, e),
            }
        }
    }

    Ok((left_count, joined_count))
}
