extern crate petgraph;
extern crate petgraph_graphml;
extern crate rand;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use std::io::prelude::*;
use std::io;
use std::fs;
use std::fmt;
use std::path::Path;
use petgraph::dot::{Dot, Config};
use petgraph_graphml::GraphMl;

use std::collections::hash_map::DefaultHasher;
use std::collections::hash_map::RandomState;
use std::hash::{Hash, Hasher, BuildHasher};
use rand::Rng;

pub type Graph = petgraph::Graph<Node, (), petgraph::Undirected>;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeType {
    Room,
    User,
    Server,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Node {
    pub kind: NodeType,
    pub id: u64,
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.kind {
            NodeType::Room => write!(f, "room_{}", self.id),
            NodeType::User => write!(f, "user_{}", self.id),
            NodeType::Server => write!(f, "server_{}", self.id),
        }
    }
}

// hack around the type signature of Dot::fmt which requires both node and edge data types to implement Display.
// But as I have no edge data, I want to use (), which does not implement Display, though.
// Convert to this type before using Dot::fmt. As I use the EdgeNoLabel option of Dot::fmt, unreachable! is enough.
struct NoEdgeData;
impl fmt::Display for NoEdgeData {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        unreachable!();
    }
}

fn hash_with_salt(builder: &BuildHasher<Hasher = DefaultHasher>, x: &impl Hash, salt: u64) -> u64 {
    let mut hasher = builder.build_hasher();
    x.hash(&mut hasher);
    salt.hash(&mut hasher);
    hasher.finish()
}

pub fn read_graph<P: AsRef<Path>>(path: P) -> Result<Graph, serde_json::Error> {
    let file = fs::File::open(path).unwrap();
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader)
}

pub fn write_graph(graph: &Graph) -> Result<(), serde_json::Error> {
    let dir = Path::new("graph");
    if !dir.exists() {
        fs::create_dir(dir).unwrap();
    }

    let file = fs::File::create("graph/graph.json").expect("Could not create graph/graph.json file");
    let writer = io::BufWriter::new(file);
    serde_json::to_writer(writer, graph)
}

pub fn export_graph_to_graphml(graph: &Graph) -> io::Result<()> {
    let dir = Path::new("graph");
    if !dir.exists() {
        fs::create_dir(dir).unwrap();
    }

    let graphml = GraphMl::new(&graph).pretty_print(true).export_node_weights_display();
    let file = fs::File::create("graph/graph.graphml").expect("Could not create graph/graph.graphml file");
    let writer = io::BufWriter::new(file);
    graphml.to_writer(writer)
}

pub fn export_graph_to_dot(graph: &Graph) -> io::Result<()> {
    let dir = Path::new("graph");
    if !dir.exists() {
        fs::create_dir(dir).unwrap();
    }

    let no_edge_data = graph.map(|_, node| node.clone(), |_, _| NoEdgeData);
    let exported_graph = Dot::with_config(&no_edge_data, &[Config::EdgeNoLabel]);
    let file = fs::File::create("graph/graph.dot").expect("Could not create graph/graph.dot file");
    let mut buffer = io::BufWriter::new(file);
    write!(&mut buffer, "{}", exported_graph)
}

pub fn anonymize_graph(graph: Graph) -> Graph {
    let hash_key = RandomState::new();
    let mut rng = rand::thread_rng();
    let salt = rng.gen::<u64>();
    graph.map(|_, node| Node { kind: node.kind, id: hash_with_salt(&hash_key, &node.id, salt)}, |_, _| ())
}
