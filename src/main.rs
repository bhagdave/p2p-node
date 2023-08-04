/*
 * Main file for p2p rendezvous server
 *
 * 
*/
use futures::StreamExt;
use std::collections::hash_map::DefaultHasher;
use libp2p::{
    core::transport::upgrade::Version,
    identify,
    identity,
    noise,
    ping,
    rendezvous,
    swarm::{keep_alive, NetworkBehaviour, SwarmEvent, SwarmBuilder},
    gossipsub,
    mdns,
    tcp,
    yamux,
    PeerId,
    Transport,
};
use std::time::Duration;
use std::hash::{Hash, Hasher};

#[tokio::main]
async fn main() {
    env_logger::init();

    // Welcome messages and info
    log::info!("Starting p2p rendezvous server - v0.1.0");

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
        }).build().expect("Valid config");

    // Setup gossipsub
    let mut gossipsub = gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Signed(local_key.clone()), gossipsub_config).expect("Correct config");

    // setup mdns
    let mdns = mdns::async_io::Behaviour::new(mdns::Config::default(), local_peer_id).expect("Correct config");

    // Create the swarm
    let mut swarm = SwarmBuilder::with_tokio_executor(
            tcp::tokio::Transport::default().upgrade(Version::V1Lazy).authenticate(noise::Config::new(&local_key).unwrap()).multiplex(yamux::Config::default()).boxed(),
            MyBehaviour {
                identify: identify::Behaviour::new(identify::Config::new("p2p-rendezvous-server/0.1.0".into(), local_key.public())),
                rendezvous: rendezvous::server::Behaviour::new(rendezvous::server::Config::default()),
                ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(1))),
                keep_alive: keep_alive::Behaviour,
                gossipsub,
                mdns,
            },
            local_peer_id,
    ).build();

    // Listen on specific port for incoming connections
    let _ = swarm.listen_on("/ip4/0.0.0.0/tcp/62649".parse().unwrap());
    log::info!("Listening on port 62649");

    // Lets catch the swarm events
    while let Some(event) = swarm.next().await {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                log::info!("Connection established with {}", peer_id);
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                log::info!("Connection with {} closed", peer_id);
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Rendezvous(rendezvous::server::Event::PeerRegistered { peer, registration },)) => {
                log::info!("Peer {} registered for namespace {:?}", peer, registration.namespace);
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Rendezvous(rendezvous::server::Event::DiscoverServed { enquirer, registrations, },)) => {
                log::info!("Served peer {} with registrations {:?}", enquirer, registrations.len());
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
                    println!("mDNS discover peer has expired: {peer_id}");
                    swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                }
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source: peer_id,
                message_id: id,
                message,
            })) => {
                log::info!("Got message: '{}' with id: {id} from peer: {peer_id}", String::from_utf8_lossy(&message.data),);
            }
            other => {
                log::info!("Swarm event: {:?}", other);
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
}
