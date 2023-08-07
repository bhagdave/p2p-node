/*
 * Main file for p2p rendezvous server
 *
 *
*/
use futures::StreamExt;
use libp2p::{
    core::transport::upgrade::Version,
    gossipsub, identify, identity, mdns, noise, ping, rendezvous,
    swarm::{keep_alive, NetworkBehaviour, SwarmBuilder, SwarmEvent},
    kad::{
        self, record::store::MemoryStore, GetProvidersOk, Kademlia, KademliaEvent, QueryId,
        QueryResult,
    },
    request_response::{self, ProtocolSupport, RequestId, ResponseChannel},
    tcp, yamux, PeerId, Transport,
};
use libp2p_quic as quic;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    env_logger::init();
    let version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");

    // Welcome messages and info
    log::info!("Starting p2p rendezvous server - {}", version);

    // Generate a random PeerId
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    log::info!("Local peer id: {:?}", local_peer_id);

    // Setup the gossipsub configuration
    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(10))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .message_id_fn(|message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        })
        .build()
        .expect("Valid config");

    // Setup gossipsub
    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .expect("Correct config");

    // setup mdns
    let mdns = mdns::async_io::Behaviour::new(mdns::Config::default(), local_peer_id)
        .expect("Correct config");

    // Setup gossipsub topic
    let topic = gossipsub::IdentTopic::new("p2p-node");
    gossipsub.subscribe(&topic).unwrap();

    // setup tcp transport
    let tcp_transport = tcp::tokio::Transport::default()
        .upgrade(Version::V1Lazy)
        .authenticate(noise::Config::new(&local_key).unwrap())
        .multiplex(yamux::Config::default())
        .timeout(Duration::from_secs(20))
        .boxed();

    // Create the swarm
    let mut swarm = SwarmBuilder::with_tokio_executor(
        tcp_transport,
        MyBehaviour {
            identify: identify::Behaviour::new(identify::Config::new(
                "p2p-rendezvous-server/0.1.0".into(),
                local_key.public(),
            )),
            rendezvous: rendezvous::server::Behaviour::new(rendezvous::server::Config::default()),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(1))),
            keep_alive: keep_alive::Behaviour,
            gossipsub,
            mdns,
            kad: Kademlia::new(
                local_peer_id.clone(),
                MemoryStore::new(local_peer_id.clone()),
            ),
        },
        local_peer_id,
    )
    .build();

    // Listen on specific port for incoming connections
    let _ = swarm.listen_on("/ip4/0.0.0.0/tcp/62649".parse().unwrap());
    log::info!("Listening on port 62649");

    let mut stdin = InteractiveStdin::new();

    println!("Add a message on the command line and press enter to send it to all peers");

    // Lets catch the swarm events
    loop {
        tokio::select! {
            line = stdin.next_line() => {
                log::info!("Publishing line: {:?}", line);
                match line {
                    Ok(line) => {
                        match line {
                            Some(line)  => {
                                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), line.as_bytes()) {
                                    log::error!("Failed to publish message: {:?}", e);
                                }
                            }
                            None => {
                                log::error!("Failed to read line");
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to read line: {:?}", e);
                    }
                }
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    log::info!("Connection established with {}", peer_id);
                }
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    log::info!("Connection with {} closed", peer_id);
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Rendezvous(
                    rendezvous::server::Event::PeerRegistered { peer, registration },
                )) => {
                    log::info!(
                        "Peer {} registered for namespace {:?}",
                        peer,
                        registration.namespace
                    );
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Rendezvous(
                    rendezvous::server::Event::DiscoverServed {
                        enquirer,
                        registrations,
                    },
                )) => {
                    log::info!(
                        "Served peer {} with registrations {:?}",
                        enquirer,
                        registrations.len()
                    );
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    log::info!("p2p-rendezvous server listening on: {}", address);
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, _multiaddr) in list {
                        log::info!("mDNS discovered a new peer: {}", peer_id);
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    }
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _multiaddr) in list {
                        log::info!("mDNS discover peer has expired: {}", peer_id);
                        swarm
                            .behaviour_mut()
                            .gossipsub
                            .remove_explicit_peer(&peer_id);
                    }
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => {
                    log::info!(
                        "Got message: '{}' with id: {id} from peer: {peer_id}",
                        String::from_utf8_lossy(&message.data),
                    );
                }
                other => {
                    log::info!("Swarm event: {:?}", other);
                }
            } 
        }
    }
}

#[derive(NetworkBehaviour)]
struct MyBehaviour {
    identify: identify::Behaviour,
    rendezvous: rendezvous::server::Behaviour,
    ping: ping::Behaviour,
    keep_alive: keep_alive::Behaviour,
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::async_io::Behaviour,
    kad: Kademlia<MemoryStore>,
}

struct InteractiveStdin {
    chan: mpsc::Receiver<std::io::Result<String>>,
}

impl InteractiveStdin {
    fn new() -> Self {
        let (send, recv) = mpsc::channel(16);
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                if send.blocking_send(line).is_err() {
                    return;
                }
            }
        });
        Self { chan: recv }
    }
    async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        self.chan.recv().await.transpose()
    }
}
