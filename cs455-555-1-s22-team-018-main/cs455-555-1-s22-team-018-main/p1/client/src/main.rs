use std::io::{BufRead, Write};

/// An error in command syntax, to be reported to the user
struct CommandError(String);

/// A connection to a server
struct Connection {
    stream: std::net::TcpStream,
}

impl Connection {
    /// Send a packet to the server
    pub fn send_packet(&mut self, packet: &chat_types::PacketC2S) -> Result<(), std::io::Error> {
        let mut bytes = serde_json::to_vec(packet).unwrap();
        bytes.push(b'\n');
        self.stream.write_all(&bytes)
    }
}

/// Handle a slash-command. Returns `false` if the command was "/quit", `true` otherwise.
fn handle_command(connection: &mut Option<Connection>, line: &str) -> Result<bool, CommandError> {
    let mut spl = line.split(' ');
    match &spl.next().unwrap()[1..] {
        "connect" => {
            const USAGE: &str = "Usage: /connect <host:port>";

            let host = spl.next().ok_or_else(|| CommandError(USAGE.to_owned()))?;

            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else if connection.is_some() {
                Err(CommandError("Already connected to a server".to_owned()))
            } else {
                println!("Connecting...");

                let stream = std::net::TcpStream::connect(host).map_err(|err| {
                    CommandError(format!("Failed to connect to server: {:?}", err))
                })?;
                let stream_clone = stream.try_clone().unwrap();

                std::thread::spawn(move || {
                    let stream = stream_clone;
                    let reader = std::io::BufReader::new(&stream);
                    for line in reader.lines() {
                        let line = match line {
                            Ok(line) => line,
                            Err(err) => {
                                eprintln!("Error in connection: {:?}", err);
                                break;
                            }
                        };

                        match serde_json::from_str(&line) {
                            Ok(packet) => {
                                use chat_types::PacketS2C;
                                match packet {
                                    PacketS2C::NewMessage { text, nick } => {
                                        println!("<{}> {}", nick, text);
                                    }
                                    PacketS2C::ListChannelsResponse { channels } => {
                                        println!("Channel List:");
                                        for channel in channels {
                                            println!(
                                                "\t{} - {} member{}",
                                                channel.name,
                                                channel.member_count,
                                                if channel.member_count == 1 { "" } else { "s" }
                                            );
                                        }
                                    }
                                    PacketS2C::ListStatsResponse {
                                        client_num,
                                        server_uptime,
                                    } => {
                                        println!("<{}> {}", client_num, server_uptime.as_secs());
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!("Failed to parse server message: {:?}", err);
                            }
                        }
                    }
                });

                *connection = Some(Connection { stream });

                println!("Connected.");

                Ok(true)
            }
        }
        "help" => {
            const USAGE: &str = "Usage: /help";

            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else {
                println!(
                    "List of commands:
/connect <host:port>
/help
/join <channel>
/leave
/list
/nick <nick>
/stats
/quit"
                );

                Ok(true)
            }
        }
        "join" => {
            const USAGE: &str = "Usage: /join <channel>";

            let channel_name = spl.next().ok_or_else(|| CommandError(USAGE.to_owned()))?;

            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else if let Some(connection) = connection {
                connection
                    .send_packet(&chat_types::PacketC2S::JoinChannel {
                        name: channel_name.to_owned(),
                    })
                    .map_err(|err| CommandError(format!("Failed to send command: {:?}", err)))?;

                Ok(true)
            } else {
                Err(CommandError("Not connected.".to_owned()))
            }
        }
        "leave" => {
            const USAGE: &str = "Usage: /leave";

            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else if let Some(connection) = connection {
                connection
                    .send_packet(&chat_types::PacketC2S::LeaveChannel)
                    .map_err(|err| CommandError(format!("Failed to send command: {:?}", err)))?;

                Ok(true)
            } else {
                Err(CommandError("Not connected.".to_owned()))
            }
        }
        "list" => {
            const USAGE: &str = "Usage: /list";

            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else if let Some(connection) = connection {
                connection
                    .send_packet(&chat_types::PacketC2S::ListChannels)
                    .map_err(|err| CommandError(format!("Failed to send command: {:?}", err)))?;

                Ok(true)
            } else {
                Err(CommandError("Not connected.".to_owned()))
            }
        }
        "nick" => {
            const USAGE: &str = "Usage: /nick <nick>";

            let new_nick = spl.next().ok_or_else(|| CommandError(USAGE.to_owned()))?;

            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else if let Some(connection) = connection {
                connection
                    .send_packet(&chat_types::PacketC2S::SetNick {
                        nick: new_nick.to_owned(),
                    })
                    .map_err(|err| CommandError(format!("Failed to send command: {:?}", err)))?;

                Ok(true)
            } else {
                Err(CommandError("Not connected.".to_owned()))
            }
        }
        "quit" => {
            const USAGE: &str = "Usage: /quit";
            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else {
                Ok(false)
            }
        }
        "stats" => {
            const USAGE: &str = "Usage: /stats";
            if spl.next().is_some() {
                Err(CommandError(USAGE.to_owned()))
            } else if let Some(connection) = connection {
                connection
                    .send_packet(&chat_types::PacketC2S::ListStats)
                    .map_err(|err| CommandError(format!("Failed to send command: {:?}", err)))?;

                Ok(true)
            } else {
                Err(CommandError("Not connected.".to_owned()))
            }
        }
        _ => Err(CommandError("Unknown command.".to_owned())),
    }
}

fn main() {
    let mut connection = None;

    let mut line = String::new();
    let stdin = std::io::stdin();
    loop {
        stdin.read_line(&mut line).unwrap();

        {
            let line = line.trim_end();

            if line.starts_with('/') {
                match handle_command(&mut connection, &line) {
                    Err(err) => {
                        eprintln!("{}", err.0);
                    }
                    Ok(true) => {}
                    Ok(false) => {
                        // quit

                        break;
                    }
                }
            } else {
                // plain message

                match &mut connection {
                    Some(connection) => {
                        let packet = chat_types::PacketC2S::NewMessage {
                            text: line.to_owned(),
                        };
                        if let Err(err) = connection.send_packet(&packet) {
                            eprintln!("Failed to send message: {:?}", err);
                        }
                    }
                    None => {
                        eprintln!("Not connected.");
                    }
                }
            }
        }

        line.clear();
    }
}
