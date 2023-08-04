/*
 * Main file for p2p rendezvous server
 *
 * 
*/
use futures::StreamExt;
use libp2p::{
    core::transport::upgrade::Version,
    identify,
    identity,
    noise,
    ping,
    rendezvous,
    swarm::{keep_alive, NetworkBehaviour, SwarmEvent, SwarmBuilder},
    tcp,
    yamux,
    PeerId,
    Transport,
};
use std::time::Duration;

#[tokio::main]
async fn main() {
    env_logger::init();

    // Welcome messages and info
    log::info!("Starting p2p rendezvous server");
    println!("p2p rendezvous server - v0.1.0");

    // Generate a random PeerId
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    log::info!("Local peer id: {:?}", local_peer_id);
    println!("Local peer id: {:?}", local_peer_id);

    // Create the swarm
    let mut swarm = SwarmBuilder::with_tokio_executor(
            tcp::tokio::Transport::default().upgrade(Version::V1Lazy).authenticate(noise::Config::new(&local_key).unwrap()).multiplex(yamux::Config::default()).boxed(),
            MyBehaviour {
                identify: identify::Behaviour::new(identify::Config::new("p2p-rendezvous-server/0.1.0".into(), local_key.public())),
                rendezvous: rendezvous::server::Behaviour::new(rendezvous::server::Config::default()),
                ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(1))),
                keep_alive: keep_alive::Behaviour,
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
}
