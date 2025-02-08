use futures::StreamExt;
use libp2p::{
    core::transport::upgrade::Version,
    gossipsub, identify, identity, mdns, noise, ping, rendezvous,
    swarm::{NetworkBehaviour, SwarmEvent, Config as SwarmConfig},
    tcp, yamux, PeerId, Transport, Swarm,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
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
    let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
        .expect("Correct config");

    // Setup gossipsub topic
    let topic = gossipsub::IdentTopic::new("p2p-node");
    gossipsub.subscribe(&topic).unwrap();

    // setup tcp transport
    let tcp_transport = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true))
        .upgrade(Version::V1Lazy)
        .authenticate(noise::Config::new(&local_key).unwrap())
        .multiplex(yamux::Config::default())
        .timeout(Duration::from_secs(20))
        .boxed();

    // Create the swarm
    let mut swarm = Swarm::new(
        tcp_transport,
        MyBehaviour {
            identify: identify::Behaviour::new(identify::Config::new(
                "p2p-rendezvous-server/0.1.0".into(),
                local_key.public(),
            )),
            rendezvous: rendezvous::server::Behaviour::new(rendezvous::server::Config::default()),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(1))),
            gossipsub,
            mdns,
        },
        local_peer_id,
        SwarmConfig::with_tokio_executor(),
    );

    // Listen on specific port for incoming connections
    let _ = swarm.listen_on("/ip4/0.0.0.0/tcp/62649".parse().unwrap());
    log::info!("Listening on port 62649");

    let (tx, mut rx) = mpsc::channel(64);
    
    // Spawn stdin handler
    tokio::spawn(async move {
        let mut stdin = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = stdin.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });

    println!("Add a message on the command line and press enter to send it to all peers");

    // Lets catch the swarm events
    loop {
        tokio::select! {
            line = rx.recv() => {
                if let Some(line) = line {
                    log::info!("Publishing line: {:?}", line);
                    if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), line.as_bytes()) {
                        log::error!("Failed to publish message: {:?}", e);
                    }
                } else {
                    break;
                }
            }
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
                    log::debug!("Unhandled swarm event: {:?}", other);
                }
            }
        }
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "MyBehaviourEvent", event_process = true)]
struct MyBehaviour {
    identify: identify::Behaviour,
    rendezvous: rendezvous::server::Behaviour,
    ping: ping::Behaviour,
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

#[derive(Debug)]
enum MyBehaviourEvent {
    Identify(identify::Event),
    Rendezvous(rendezvous::server::Event),
    Ping(ping::Event),
    Gossipsub(gossipsub::Event),
    Mdns(mdns::Event),
}

impl From<identify::Event> for MyBehaviourEvent {
    fn from(event: identify::Event) -> Self {
        MyBehaviourEvent::Identify(event)
    }
}

impl From<rendezvous::server::Event> for MyBehaviourEvent {
    fn from(event: rendezvous::server::Event) -> Self {
        MyBehaviourEvent::Rendezvous(event)
    }
}

impl From<ping::Event> for MyBehaviourEvent {
    fn from(event: ping::Event) -> Self {
        MyBehaviourEvent::Ping(event)
    }
}

impl From<gossipsub::Event> for MyBehaviourEvent {
    fn from(event: gossipsub::Event) -> Self {
        MyBehaviourEvent::Gossipsub(event)
    }
}

impl From<mdns::Event> for MyBehaviourEvent {
    fn from(event: mdns::Event) -> Self {
        MyBehaviourEvent::Mdns(event)
    }
}
