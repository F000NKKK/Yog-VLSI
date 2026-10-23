//! What the screen gets and sends.

use super::{AluId, Node, Pin, REQUEST_CHANNEL, Wire};
use crate::content::Tier;
use crate::vm::PortMode;
use yog_api::Schema;

#[derive(Debug, Clone, PartialEq, Schema)]
pub struct ChipView {
    pub slot: u8,
    pub name: String,
    pub tier: Tier,
    pub pins: Vec<(String, PortMode)>,
    /// In a slot the ALU's tier powers.
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Schema)]
pub struct SideView {
    pub side: String,
    /// The Redstone Port at the other end of its cable.
    pub name: Option<String>,
    pub output: bool,
}

/// A bus node: the side it's on, how many channels the tier sees, how
/// many rows it shows.
#[derive(Debug, Clone, PartialEq, Schema)]
pub struct BusView {
    pub index: u8,
    pub side: String,
    pub channels: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Schema)]
pub struct BoardView {
    pub alu: AluId,
    pub tier: Tier,
    pub chips: Vec<ChipView>,
    pub sides: Vec<SideView>,
    pub buses: Vec<BusView>,
    pub wires: Vec<Wire>,
    /// Where the player put the nodes (chips, the In and Out panels).
    pub layout: Vec<(Node, (f32, f32))>,
    pub levels: Vec<(Pin, u8)>,
}

#[derive(Debug, Clone, Schema)]
pub enum Request {
    Connect {
        alu: AluId,
        from: Pin,
        to: Pin,
    },
    Disconnect {
        alu: AluId,
        to: Pin,
    },
    Clear {
        alu: AluId,
    },
    Move {
        alu: AluId,
        node: Node,
        x: f32,
        y: f32,
    },
    /// Pulls a bus node open to `rows` channels (or closes it to them).
    BusRows {
        alu: AluId,
        side: u8,
        rows: u16,
    },
}

impl Request {
    pub(super) fn alu(&self) -> &AluId {
        match self {
            Request::Connect { alu, .. }
            | Request::Disconnect { alu, .. }
            | Request::Clear { alu }
            | Request::Move { alu, .. }
            | Request::BusRows { alu, .. } => alu,
        }
    }
}

/// Sends `request` to the server (from the screen).
pub fn send(request: &Request) {
    let bytes = yog_api::yog_data::to_bytes(request);
    yog_api::event::net::send_to_server(REQUEST_CHANNEL, &bytes);
}
