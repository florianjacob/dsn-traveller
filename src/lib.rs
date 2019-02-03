#![feature(try_from)]
// enable the await! macro, async support, and the new std::Futures api.
#![feature(await_macro, async_await, futures_api)]

use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt;
use std::iter::FromIterator;
use std::time;

use tokio::await;

use futures_timer::Delay;
use ruma_client::api::r0;
use ruma_client::Client;
use ruma_events::room::member::MembershipState;
use ruma_events::room::message::{MessageEventContent, MessageType, TextMessageEventContent};
use ruma_events::stripped::StrippedState;
use ruma_events::EventType;
use ruma_identifiers::{EventId, RoomAliasId, RoomId, RoomIdOrAliasId, UserId};

use petgraph::prelude::*;

use hyper::client::connect::Connect;

use lazy_static::lazy_static;

use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

use matrixgraph::{Node, NodeType};

// if we continue to use the same access token,
// we need to try to have unique txnids.
// alternatively, we could store the last txnid on shutdown
lazy_static! {
    static ref TXN_ID: AtomicUsize = {
        let mut rng = rand::thread_rng();
        let id = rng.gen();
        AtomicUsize::new(id)
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

// this is essentially a ruma_identifiers::UserId without localpart,
// to profit from the UserId parsing rules and being easily able to differentiate servers if they
// have  non-standard port numbers
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ServerId {
    hostname: url::Host,
    port: u16,
}

impl ServerId {
    fn new(user_id: &UserId) -> Self {
        ServerId {
            hostname: user_id.hostname().clone(),
            port: user_id.port(),
        }
    }
}
impl fmt::Display for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.hostname, self.port)
    }
}

pub async fn send_message<C: Connect + 'static>(
    client: Client<C>,
    room_id: RoomId,
    message: String,
) -> Result<EventId, ruma_client::Error> {
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

async fn joined_rooms<C: Connect + 'static>(
    client: Client<C>,
) -> Result<Vec<RoomId>, ruma_client::Error> {
    use r0::membership::joined_rooms;
    let response = await!(joined_rooms::call(client.clone(), joined_rooms::Request {}))?;
    Ok(response.joined_rooms)
}

async fn sync_rooms<C: Connect + 'static>(
    client: Client<C>,
) -> Result<r0::sync::sync_events::Rooms, ruma_client::Error> {
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

    use r0::sync::sync_events;
    let response = await!(sync_events::call(
        client.clone(),
        sync_events::Request {
            filter: Some(sync_events::Filter::FilterDefinition(filter_definition)),
            since: None,
            full_state: Some(true),
            set_presence: None,
            timeout: None,
        }
    ))
    .expect("Could not get sync response");
    eprintln!("next batch: {}", response.next_batch);
    Ok(response.rooms)
}

pub async fn join_rooms<C: Connect + 'static>(
    client: Client<C>,
    room_aliases: Vec<RoomAliasId>,
) -> Result<(usize, usize, usize), ruma_client::Error> {
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
                    eprintln!(
                        "Followed invite to room: {:?} ({}/{})",
                        canonical_alias.clone(),
                        invite_count,
                        invites_to_follow
                    );
                },
                Err(e) => eprintln!("Error joining invited room {}: {:?}", canonical_alias, e),
            };
        } else {
            // this seem to be mostly invites from NickServ bots or similar from IRC bridges
            // -> one can directly follow invites by ID, as the inviting server is already known, it seems!
            // TODO: directly join by id and skip canonical alias stuff from above?
            eprintln!(
                "could resolve canonical alias for invited room {:#?}, trying to join by room id",
                invite
            );
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
                    eprintln!(
                        "Followed invite to room: {:?} ({}/{})",
                        room_id.clone(),
                        invite_count,
                        invites_to_follow
                    );
                },
                Err(e) => eprintln!("Error joining invited room through id{:?}: {:?}", invite, e),
            };
        }
    }

    if room_aliases.is_empty() {
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
        if !joined_rooms_set.contains(&room_id)
            && !invited_rooms_set.contains(&room_id)
            && !left_rooms_set.contains(&room_id)
        {
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
                    eprintln!(
                        "Joined room: {:?} ({}/{})",
                        alias, join_count, rooms_to_join
                    );
                },
                Err(e) => eprintln!("Error joining room {}: {:?}", room_id, e),
            };

            await!(Delay::new(ROOM_JOIN_DELAY)).expect("wait failed");
        } else {
            eprintln!(
                "already joined, invited or was kicked from room {}.",
                room_id
            );
        }
    }
    Ok((join_count, invite_count, rooms.leave.len()))
}

async fn leave_and_forget_room<C: Connect + 'static>(
    client: Client<C>,
    room_id: RoomId,
) -> Result<(), ruma_client::Error> {
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
        forget_room::Request { room_id }
    ))?;
    Ok(())
}

pub async fn resolve_alias<C: Connect + 'static>(
    client: Client<C>,
    room_alias: RoomAliasId,
) -> Result<RoomId, ruma_client::Error> {
    use r0::alias::get_alias;
    let response = await!(get_alias::call(
        client.clone(),
        get_alias::Request { room_alias }
    ))?;
    Ok(response.room_id)
}

pub async fn into_room_id<C: Connect + 'static>(
    client: Client<C>,
    room_id_or_alias_id: RoomIdOrAliasId,
) -> Result<RoomId, ruma_client::Error> {
    match room_id_or_alias_id {
        RoomIdOrAliasId::RoomId(room_id) => Ok(room_id),
        RoomIdOrAliasId::RoomAliasId(alias) => await!(resolve_alias(client.clone(), alias)),
    }
}

/// delivers the user ids of all users currently joined in the given room
async fn room_members<C: Connect + 'static>(
    client: Client<C>,
    room_id: RoomId,
) -> Result<Vec<String>, ruma_client::Error> {
    use r0::sync::get_member_events;
    let response = await!(get_member_events::call(
        client.clone(),
        get_member_events::Request {
            room_id: room_id.clone(),
        }
    ))?;

    // in the case of join membership events it's probably always the case that sender is the same user
    // the event relates to, but actually, the state key is the field building the relationship to the user.
    let state_keys = response
        .chunk
        .into_iter()
        .filter(|event| event.content.membership == MembershipState::Join)
        .map(|event| event.state_key)
        .collect();
    Ok(state_keys)
}

fn hash(builder: &BuildHasher<Hasher = DefaultHasher>, x: &impl Hash) -> u64 {
    let mut hasher = builder.build_hasher();
    x.hash(&mut hasher);
    hasher.finish()
}

pub async fn crawl<C: Connect + 'static>(
    client: Client<C>,
) -> Result<(usize, usize, usize), ruma_client::Error> {
    // * ignore ourself and voyager, as we are in all rooms but silent, so we won't send messages in the simulation
    // * weho.st and disroot.org requested to opt out as whole server, this will lead to an
    //   anonymized graph in which those servers and the users on them never existed.
    let member_ignore_pattern = regex::Regex::new(
        r"^(@.*:dsn-traveller.dsn.scc.kit.edu|@voyager:t2bot.io|@.*:weho.st|@.*:disroot.org)$",
    )
    .unwrap();

    let joined_rooms = await!(joined_rooms(client.clone()))?;
    let mut graph: Graph<Node, (), petgraph::Undirected> = Graph::new_undirected();

    let mut room_indexes = HashMap::<RoomId, NodeIndex>::new();
    let mut user_indexes = HashMap::<UserId, NodeIndex>::new();
    let mut server_indexes = HashMap::<ServerId, NodeIndex>::new();

    // pseudonymization:
    // on each crawl, choose a different random has function
    let hash_key = RandomState::new();
    let mut crawled_rooms = 0;
    let rooms_to_crawl = joined_rooms.len();

    for room in joined_rooms {
        await!(Delay::new(ROOM_CRAWL_DELAY)).expect("wait failed");

        // occasionally this resulted in a bad gateway error
        // could not find the synapse log lines for that, but it's probably due to server overload.
        // redoing it once worked fine.
        let members = match await!(room_members(client.clone(), room.clone())) {
            Ok(members) => members,
            Err(e) => {
                eprintln!("error getting room members: {:?}, retrying once.", e);
                await!(room_members(client.clone(), room.clone()))?
            },
        };

        for member in members {
            if member_ignore_pattern.is_match(member.as_str()) {
                continue;
            }
            // if we came as far as here, there's at least one non-ignored user in that room, and
            // we can add it to the graph.
            let room_idx = room_indexes.entry(room.clone()).or_insert_with(|| {
                graph.add_node(Node {
                    kind: NodeType::Room,
                    id: hash(&hash_key, &room),
                })
            });

            let user_id = UserId::try_from(member.as_str()).unwrap();
            let server_id = ServerId::new(&user_id);
            let is_new_server = !server_indexes.contains_key(&server_id);
            let server_idx = server_indexes.entry(server_id.clone()).or_insert_with(|| {
                graph.add_node(Node {
                    kind: NodeType::Server,
                    id: hash(&hash_key, &server_id),
                })
            });

            // is_new_server -> !user_indexes.contains_key,
            // if this is a new server, it can't have users yet
            debug_assert!(
                !is_new_server || !user_indexes.contains_key(&user_id),
                "Server {} is new, but we already found User {}!",
                server_id,
                user_id
            );
            let user_idx = user_indexes.entry(user_id.clone()).or_insert_with(|| {
                let user_idx = graph.add_node(Node {
                    kind: NodeType::User,
                    id: hash(&hash_key, &user_id),
                });
                graph.add_edge(user_idx, *server_idx, ());
                user_idx
            });

            graph.add_edge(*user_idx, *room_idx, ());
            // connect room and the user's server in case that edge was not yet there
            graph.update_edge(*server_idx, *room_idx, ());
        }
        crawled_rooms += 1;
        eprintln!("Crawled {}/{} rooms", crawled_rooms, rooms_to_crawl);
    }

    assert!(matrixgraph::is_wellformed_graph(&graph));

    let graph = matrixgraph::anonymize_graph(graph);

    let dir = matrixgraph::graph_dir();
    matrixgraph::write_graph(&graph, &dir).unwrap();
    matrixgraph::export_graph_to_dot(&graph, &dir).unwrap();
    matrixgraph::export_graph_to_graphml(&graph, &dir).unwrap();

    Ok((room_indexes.len(), user_indexes.len(), server_indexes.len()))
}

pub async fn exit_all<C: Connect + 'static>(
    client: Client<C>,
    control_room: RoomId,
) -> Result<(usize, usize), ruma_client::Error> {
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
                    eprintln!("Left room: {} ({}/{})", room_id, left_count, joined_count);
                },
                Err(e) => eprintln!("Error leaving / forgetting room {}: {:?}", room_id, e),
            }
        }
    }

    Ok((left_count, joined_count))
}

pub async fn exit<C: Connect + 'static>(
    client: Client<C>,
    room_id: RoomId,
) -> Result<(), ruma_client::Error> {
    // leaving as well as forgetting so that the server could part the federation for that rooms.
    // Also, if we would not forget leaved rooms, they would appear as rooms where the bot has been
    // kicked from on a later join run.
    // TODO: is leave_and_forget_room enough so that the server can be shut down
    // without being a dead member of the federation?
    match await!(leave_and_forget_room(client.clone(), room_id.clone())) {
        Ok(_) => {
            eprintln!("Left room: {}", room_id);
            Ok(())
        },
        Err(e) => {
            eprintln!("Error leaving / forgetting room {}: {:?}", room_id, e);
            Err(e)
        },
    }
}
