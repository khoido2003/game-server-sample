use std::{
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use cgmath::Vector2;
use tokio::{net::UdpSocket, sync::Mutex};

use egui::ahash::{HashMap, HashMapExt};
use game_server_sample::{generate_color, globals, Player, PlayerId};
use tokio::sync::mpsc;

use crate::message::{self, Message};

/////////////////////////////////////////////

// Store user connected in a hashmap
type PlayerMap = HashMap<SocketAddr, Player>;

// Define message and channel
struct BroadcastMessage {
    msg: Vec<u8>,
    excluded_client: Option<SocketAddr>,
}
type ChannelSender = mpsc::UnboundedSender<BroadcastMessage>;
type ChannelReceiver = mpsc::UnboundedReceiver<BroadcastMessage>;

// Define Server
struct ServerContext {
    server_socket: UdpSocket,
    broadcast_tx: ChannelSender,
    players: Mutex<PlayerMap>,
    player_id_counter: AtomicU64,
}

impl ServerContext {
    fn new(server_socket: UdpSocket, broadcast_tx: ChannelSender) -> Self {
        Self {
            server_socket,
            broadcast_tx,
            players: Mutex::new(PlayerMap::new()),
            player_id_counter: AtomicU64::new(1),
        }
    }
}

//////////////////////////////////////////////////////

// Network method

// Receive message from udp
async fn listen_handler(context: Arc<ServerContext>) {
    loop {
        let mut buf = [0u8; 32];
        // NOTE: consider using non-blocking I/O UDP
        let (len, client) = context.server_socket.recv_from(&mut buf).await.unwrap();

        if len > 1 {
            let request_msg = String::from_utf8_lossy(&buf[..len]).to_string();

            tokio::spawn(process_client_message(context.clone(), client, request_msg));
        }
    }
}

// Sender loop to response to all players except the player who owning the broadcast message
async fn broadcast_sender(context: Arc<ServerContext>, mut broadcast_rx: ChannelReceiver) {
    while let Some(broadcast) = broadcast_rx.recv().await {
        message::trace(format!(
            "Broadcasting: {}",
            String::from_utf8_lossy(&broadcast.msg)
        ));

        let players = context.players.lock().await;

        for (client_addr, _) in players.iter() {
            if Some(*client_addr) != broadcast.excluded_client {
                if let Err(e) = context
                    .server_socket
                    .send_to(&broadcast.msg, client_addr)
                    .await
                {
                    eprintln!("Failed to broadcast: {:?}", e);
                }
            }
        }
    }
}

// Healthcheck for server
async fn ping_sender(context: Arc<ServerContext>) {
    let mut interval = tokio::time::interval(globals::PING_INTERVAL_MS);

    loop {
        interval.tick().await;
        let _ = context.broadcast_tx.send(BroadcastMessage {
            msg: Message::Ping.serialize().into_bytes(),
            excluded_client: None,
        });
    }
}

/// Authoritative game update logic simulation
///
/// Required fixed processing, because timing has to be synchronized accross all the connected
/// clients. A server simulation loop does not need to play "catch-up" like a local game loop does
/// because there no point in sending stale state
async fn simulation_handler(context: Arc<ServerContext>) {
    let desired_frame_duration =
        std::time::Duration::from_secs_f32(globals::FIXED_UPDATE_TIMESTEP_SEC);

    let mut interval = tokio::time::interval(desired_frame_duration);

    interval.tick().await;

    loop {
        let current_time = std::time::Instant::now();

        // Add new scope here so when finish the lock will be release
        {
            let mut players = context.players.lock().await;
            for (client_addr, player) in players.iter_mut() {
                // Bound checking
                globals::clamp_player_to_bounds(player);

                // Gameplay state replication
                let msg = Message::Replicate(*player).serialize();

                let _ = context.broadcast_tx.send(BroadcastMessage {
                    msg: msg.into_bytes(),
                    excluded_client: Some(*client_addr),
                });
            }
        }

        // Calcualte the time has passed, if the update happendes too fast then the
        // tick will wait until the next tick to continue the loop
        let elapsed_time = current_time.elapsed();
        if elapsed_time < desired_frame_duration {
            interval.tick().await;
        }
    }
}

//////////////////////////////////////////////

// Proccessing client request
async fn process_client_message(context: Arc<ServerContext>, client: SocketAddr, msg: String) {
    // If trace enable then log the trace
    message::trace(format!("Received: {msg}"));

    match Message::deserialize(&msg) {
        Ok(Message::Handshake) => {
            if let Err(e) = accept_client(context.clone(), client).await {
                eprintln!("Error accepting client {}: {}", client, e);
            }
        }

        Ok(Message::Position(player_id, pos)) => {
            if let Err(e) = update_position(context, client, player_id, pos).await {
                eprintln!("Error updating player position {}: {}", player_id, e);
            }
        }

        Ok(Message::Leave(player_id)) => {
            if let Err(e) = drop_player(context.clone(), client, player_id).await {
                eprintln!("Error dropping player {}: {}", player_id, e);
            }
        }

        _ => (),
    }
}

// Accept client connect
async fn accept_client(
    context: Arc<ServerContext>,
    client: SocketAddr,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut players = context.players.lock().await;

    let ack_msg: String;
    if let Some(existing_player) = players.get(&client) {
        // Getting multiple handshakes from and sending out multiple ACK for the same
        // client is not a problem, that just means that previous ACK was dropped, so the
        // client retried the HANDSHAKE. Server just resends ACK with same player info that
        // was already registered as response to new HANDSHAKE. It is made sure here not to
        // accidentally add the same player multiple times, because that would lead to
        // "Player 3 joined, Player
        // 4 joined, Player 5 joined" bug for each accepted HANDSHAKE from the same client.
        ack_msg = Message::Ack(existing_player.id, existing_player.color).serialize();
    } else {
        let new_player = Player::new(
            context.player_id_counter.fetch_add(1, Ordering::SeqCst),
            generate_color(),
        );

        players.insert(client, new_player);

        // First time game startup: Start sending PING message to everyone and start
        // the game simulation when the first player
        // connected

        if players.len() == 1 {
            tokio::spawn(ping_sender(context.clone()));
            tokio::spawn(simulation_handler(context.clone()));
        }

        ack_msg = Message::Ack(new_player.id, new_player.color).serialize();
    }

    context
        .server_socket
        .send_to(ack_msg.as_bytes(), client)
        .await?;

    message::trace(format!("Sent: {ack_msg}"));

    Ok(())
}

// Update position
async fn update_position(
    context: Arc<ServerContext>,
    client: SocketAddr,
    player_id: PlayerId,
    new_pos: Vector2<f32>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(player) = context.players.lock().await.get_mut(&client) {
        if player_id != player.id {
            return Ok(());
        }

        player.pos.x = new_pos.x;
        player.pos.y = new_pos.y;
    }

    Ok(())
}

// Remove client when disconnect
async fn drop_player(
    context: Arc<ServerContext>,
    client: SocketAddr,
    player_id: PlayerId,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut players = context.players.lock().await;
    players.remove(&client);

    println!("Player {player_id} left the server");

    context.broadcast_tx.send(BroadcastMessage {
        msg: Message::Leave(player_id).serialize().into_bytes(),
        excluded_client: Some(client),
    })?;

    Ok(())
}

///////////////////////////////////////////////////

pub type ServerSessionResult = Result<(), Box<dyn Error + Send + Sync>>;
pub async fn start_server(port: u16) -> ServerSessionResult {
    match tokio::time::timeout(globals::CONNECTION_TIMEOUT_SEC, async {
        let addr = format!("0.0.0.0:{port}");

        let server_socket = UdpSocket::bind(&addr).await?;
        let (broadcast_tx, broadcast_rx) = mpsc::unbounded_channel::<BroadcastMessage>();

        let context = Arc::new(ServerContext::new(server_socket, broadcast_tx.clone()));

        // Spawn task for listen message
        tokio::spawn(listen_handler(context.clone()));

        // Broadcase message to other client
        tokio::spawn(broadcast_sender(context.clone(), broadcast_rx));

        Ok(()) as ServerSessionResult
    })
    .await
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "Server creation time out after {} seconds: {e}",
            globals::CONNECTION_TIMEOUT_SEC.as_secs()
        )
        .into()),
    }
}
