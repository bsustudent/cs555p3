#![warn(missing_docs)]
//! Crate containing shared types for chat_server and chat_client

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone)]
/// Packet sent from client to server
pub enum PacketC2S {
    /// Send a message to the current channel.
    NewMessage {
        /// Content of message
        text: String,
    },
    /// Set your nick.
    SetNick {
        /// New nick
        nick: String,
    },
    /// Join a new channel (if not already joined) and set it as the current channel.
    JoinChannel {
        /// Name of channel to join
        name: String,
    },
    /// Leave the current channel.
    LeaveChannel,
    /// Request channel list. Server will respond with [PacketS2C::ListChannelsResponse]
    ListChannels,
    /// Request server stats. Server will respond with [PacketS2C::ListStatsResponse]
    ListStats,
}

#[derive(Serialize, Deserialize, Clone)]
/// Channel info, part of [PacketS2C::ListChannelsResponse]
pub struct ListChannelInfo {
    /// Name of the channel
    pub name: String,
    /// Number of members
    pub member_count: u32,
}

#[derive(Serialize, Deserialize, Clone)]
/// Packet sent from server to client
pub enum PacketS2C {
    /// Notification of new message
    NewMessage {
        /// Nick of message sender
        nick: String,
        /// Content of message
        text: String,
    },
    /// Response to [PacketC2S::ListChannels]
    ListChannelsResponse {
        /// List of channels
        channels: Vec<ListChannelInfo>,
    },
    /// Response to [PacketC2S::ListStats]
    ListStatsResponse {
        /// Number of clients currently connected
        client_num: u32,
        /// Time since the server started
        server_uptime: Duration,
    },
}
