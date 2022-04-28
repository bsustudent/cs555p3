use ctrlc;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use std::collections::{HashMap, HashSet};
use std::process;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// Internal packet used for communicating with client handler tasks.
enum ChanPacket {
    /// Send a packet to the client
    S2C(chat_types::PacketS2C),

    /// Connection is closing
    End,
}

/// Structure used in [ServerState] to reference a client
struct ClientState {
    /// Sender of [ChanPacket]s
    sender: tokio::sync::mpsc::UnboundedSender<ChanPacket>,
}

/// State of a channel
struct ChannelState {
    /// Set of IDs of clients that are currently members
    members: HashSet<u64>,
}

/// State of the server
struct ServerState {
    /// Last ID assigned to a client. Incremented when each new client joins.
    last_id: u64,

    /// Map of client IDs to states
    clients: HashMap<u64, ClientState>,

    /// Map of channel names to states
    channels: HashMap<String, ChannelState>,

    /// Time of last message sent in any channel
    last_activity_time: SystemTime,

    /// Startup time of the server
    server_uptime: SystemTime,
}

/// Convenience type for passing around a smart pointer to the [ServerState]
type ServerStateLock = Arc<RwLock<ServerState>>;

/// Shut down the server after informing users
fn graceful_shutdown(state: ServerStateLock) {
    let text = "Server is shutting down NOW!".to_string();

    let out_packet = chat_types::PacketS2C::NewMessage {
        text,
        nick: "Server".to_string(),
    };

    {
        let state = state.read().unwrap();

        for (_, client) in &state.clients {
            let _ = client.sender.send(out_packet.clone().into());
            // if this fails then the client must have died
        }
    }
    std::thread::sleep(Duration::from_secs(1));
    process::exit(0);
}

/// Loop to shut down the server if 5 minutes pass without a message
async fn idle_shutdown(state: ServerStateLock) {
    loop {
        let last_activity_time = state.read().unwrap().last_activity_time;
        match last_activity_time.elapsed() {
            Ok(elapsed) => {
                if elapsed > Duration::from_secs(300) {
                    graceful_shutdown(state.clone());
                } else {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
            Err(e) => {
                eprintln!("Error: {:?}", e);
            }
        }
    }
}

/// Handle a client connection
async fn handle_connection_inner(
    state: ServerStateLock,
    socket: tokio::net::TcpStream,
    debug_level: u8,
) -> Result<(), anyhow::Error> {
    struct ConnectionState {
        nick: String,
        current_channel: Option<String>,
    }

    impl From<chat_types::PacketS2C> for ChanPacket {
        fn from(src: chat_types::PacketS2C) -> ChanPacket {
            ChanPacket::S2C(src)
        }
    }

    let framed = tokio_util::codec::Framed::new(socket, tokio_util::codec::LinesCodec::new());
    let (mut send, recv) = framed.split::<String>();

    let (chan_send, mut chan_recv) = tokio::sync::mpsc::unbounded_channel::<ChanPacket>();

    let my_id = {
        let mut state = state.write().unwrap();
        state.last_id += 1;
        let my_id = state.last_id;

        state.clients.insert(
            my_id,
            ClientState {
                sender: chan_send.clone(),
            },
        );

        my_id
    };

    if debug_level >= 1 {
        println!("Client {} connected", my_id);
    }

    futures_util::try_join!(
        async {
            while let Some(packet) = chan_recv.recv().await {
                match packet {
                    ChanPacket::S2C(packet) => match serde_json::to_string(&packet) {
                        Ok(line) => {
                            send.send(line).await?;
                        }
                        Err(err) => {
                            eprintln!("Failed to serialize packet: {:?}", err);
                        }
                    },
                    ChanPacket::End => break,
                }
            }

            Result::<_, anyhow::Error>::Ok(())
        },
        {
            let state = state.clone();
            async move {
                let state = &state;

                let chan_send = &chan_send;

                recv.try_fold(
                    ConnectionState {
                        nick: "".to_owned(),
                        current_channel: None,
                    },
                    |mut conn_state, line| async move {
                        match serde_json::from_str(&line) {
                            Ok(packet) => {
                                use chat_types::PacketC2S;
                                match packet {
                                    PacketC2S::NewMessage { text } => {
                                        match &conn_state.current_channel {
                                            Some(channel_name) => {
                                                {
                                                    let mut state = state.write().unwrap();
                                                    state.last_activity_time = SystemTime::now();
                                                }

                                                let out_packet =
                                                    chat_types::PacketS2C::NewMessage {
                                                        text,
                                                        nick: conn_state.nick.clone(),
                                                    };

                                                let state = state.read().unwrap();

                                                for member in &state
                                                    .channels
                                                    .get(channel_name)
                                                    .unwrap()
                                                    .members
                                                {
                                                    let client = state.clients.get(member).unwrap();
                                                    let _ = client
                                                        .sender
                                                        .send(out_packet.clone().into());
                                                    // if this fails then the client must have died
                                                }
                                            }
                                            None => {
                                                eprintln!("Client tried to send without a channel");
                                            }
                                        }
                                    }
                                    PacketC2S::SetNick { nick } => {
                                        conn_state.nick = nick;
                                    }
                                    PacketC2S::JoinChannel { name } => {
                                        {
                                            (*state.write().unwrap())
                                                .channels
                                                .entry(name.clone())
                                                .or_insert_with(|| ChannelState {
                                                    members: Default::default(),
                                                })
                                                .members
                                                .insert(my_id);
                                        }

                                        conn_state.current_channel = Some(name);
                                    }
                                    PacketC2S::LeaveChannel => {
                                        if let Some(name) = &conn_state.current_channel {
                                            {
                                                if let Some(chan_state) =
                                                    (*state.write().unwrap()).channels.get_mut(name)
                                                {
                                                    chan_state.members.remove(&my_id);
                                                }
                                            }

                                            conn_state.current_channel = None;
                                        }
                                    }
                                    PacketC2S::ListChannels => {
                                        let list = state
                                            .read()
                                            .unwrap()
                                            .channels
                                            .iter()
                                            .map(|(name, chan_state)| chat_types::ListChannelInfo {
                                                name: name.to_owned(),
                                                member_count: chan_state.members.len() as u32,
                                            })
                                            .collect();

                                        let out_packet =
                                            chat_types::PacketS2C::ListChannelsResponse {
                                                channels: list,
                                            };

                                        let _ = chan_send.send(out_packet.into());
                                    }
                                    PacketC2S::ListStats => {
                                        let state = state.read().unwrap();

                                        let client_num = state.clients.len() as u32;

                                        let out_packet = chat_types::PacketS2C::ListStatsResponse {
                                            client_num: client_num,
                                            server_uptime: state.server_uptime.elapsed().unwrap(),
                                        };

                                        let _ = chan_send.send(out_packet.into());
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!("Failed to parse packet: {:?}", err);
                            }
                        }

                        Ok(conn_state)
                    },
                )
                .await?;

                let _ = chan_send.send(ChanPacket::End);

                Ok(())
            }
        },
    )?;

    {
        let mut state = state.write().unwrap();

        for (_, chan_state) in state.channels.iter_mut() {
            chan_state.members.remove(&my_id);
        }

        state.clients.remove(&my_id);
    }

    Ok(())
}

/// Wrapper for [handle_connection_inner] to log errors
async fn handle_connection(state: ServerStateLock, socket: tokio::net::TcpStream, debug_level: u8) {
    if let Err(err) = handle_connection_inner(state, socket, debug_level).await {
        eprintln!("Error in connection: {:?}", err);
    }
}

#[derive(clap::Parser)]
/// Command-line arguments
struct Args {
    #[clap(short, default_value_t = 5005)]
    port: u16,

    #[clap(short, default_value_t = 0)]
    debug_level: u8,
}

#[tokio::main]
async fn main() {
    let Args { port, debug_level } = clap::Parser::parse();

    let listener = tokio::net::TcpListener::bind(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::UNSPECIFIED,
        port,
    ))
    .await
    .expect("Failed to initialize server");

    let state = Arc::new(RwLock::new(ServerState {
        last_id: 0,
        clients: Default::default(),
        channels: Default::default(),
        last_activity_time: SystemTime::now(),
        server_uptime: SystemTime::now(),
    }));

    tokio::spawn(idle_shutdown(state.clone()));

    {
        let state = state.clone();
        ctrlc::set_handler(move || {
            graceful_shutdown(state.clone());
        })
        .expect("Error setting Ctrl-C handler");
    }

    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                tokio::spawn(handle_connection(state.clone(), socket, debug_level));
            }
            Err(err) => {
                eprintln!("Failed to accept connection: {:?}", err);
            }
        }
    }
}
