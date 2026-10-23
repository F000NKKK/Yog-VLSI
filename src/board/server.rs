//! The server side: every ALU's board, kept, ticked, saved and shown to
//! whoever has its screen open.

use super::{
    AluId, Board, BoardView, Node, Pin, REQUEST_CHANNEL, Request, SEND_EVERY, SIDES, SYNC_EVERY,
    Saved, TICK_BUDGET, VIEW_CHANNEL, Wire,
};
use crate::cables;
use crate::content::Tier;
use crate::vm::NestedPin;
use std::collections::HashMap;
use yog_api::sync::{LazyLock, Mutex};
use yog_api::{BlockPos, EventPhase, Registry};

pub(super) static BOARDS: LazyLock<Mutex<HashMap<AluId, Board>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn side_index(name: &str) -> Option<u8> {
    SIDES.iter().position(|s| *s == name).map(|i| i as u8)
}

/// A board pin as a nested ALU's (`None` for a bus: no cables inside a chip).
pub(super) fn nested_pin(pin: &Pin) -> Option<NestedPin> {
    Some(match pin.node {
        Node::In => NestedPin::In(side_index(&pin.name)?),
        Node::Out => NestedPin::Out(side_index(&pin.name)?),
        Node::Chip(slot) => NestedPin::Chip {
            slot,
            label: pin.name.clone(),
        },
        // A bus pin is its channel's number, from 1.
        Node::Bus(side) => NestedPin::Bus {
            side,
            channel: pin.name.parse::<u16>().ok()?.checked_sub(1)?,
        },
    })
}

/// The wires of the ALU at `alu`, as an ALU inside a chip keeps them (a
/// design is saved with it in it).
pub fn nested_wires(alu: &AluId) -> Vec<(NestedPin, NestedPin)> {
    BOARDS
        .lock()
        .get(alu)
        .map(|b| {
            b.saved
                .wires
                .iter()
                .filter_map(|w| Some((nested_pin(&w.from)?, nested_pin(&w.to)?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Gives the ALU at `alu` the board `wires` (a design with an ALU in it
/// opened in the editor).
pub fn restore(alu: AluId, tier: Tier, wires: &[(NestedPin, NestedPin)]) {
    let pin = |p: &NestedPin| match p {
        NestedPin::In(i) => Pin::new(Node::In, SIDES[*i as usize % 6]),
        NestedPin::Out(i) => Pin::new(Node::Out, SIDES[*i as usize % 6]),
        NestedPin::Chip { slot, label } => Pin::new(Node::Chip(*slot), label.as_str()),
        NestedPin::Bus { side, channel } => Pin::new(Node::Bus(*side), (channel + 1).to_string()),
    };
    let mut board = Board::new(tier);
    board.saved.wires = wires
        .iter()
        .map(|(f, t)| Wire {
            from: pin(f),
            to: pin(t),
        })
        .collect();
    BOARDS.lock().insert(alu, board);
}

/// Forgets the boards of the ALUs inside `lo`..`hi` of `dimension` (an
/// editor field going).
pub fn forget_in(dimension: &str, lo: BlockPos, hi: BlockPos) {
    BOARDS.lock().retain(|a, _| {
        !(a.dimension == dimension
            && (lo.x..=hi.x).contains(&a.x)
            && (lo.y..=hi.y).contains(&a.y)
            && (lo.z..=hi.z).contains(&a.z))
    });
}
/// Who has which ALU open, and what they were last sent.
pub(super) type Watching = (AluId, Option<BoardView>);
pub(super) static WATCHERS: LazyLock<Mutex<HashMap<String, Watching>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(super) static TICKS: Mutex<u64> = Mutex::new(0);

pub fn tick() {
    let n = {
        let mut t = TICKS.lock();
        *t += 1;
        *t
    };
    let names = if n % SYNC_EVERY == 0 {
        Some(cables::port_names())
    } else {
        None
    };
    let cables_changed = cables::cables_changed();
    {
        let mut boards = BOARDS.lock();
        // Each a share of what's left of the tick's budget, starting each
        // tick one further along: no ALU always last.
        let started = std::time::Instant::now();
        let count = boards.len().max(1);
        let first = (n as usize) % count;
        let mut order: Vec<_> = boards.iter_mut().collect();
        let len = order.len();
        order.rotate_left(first.min(len));
        let mut left = count;
        for (alu, board) in order {
            let spent = started.elapsed();
            let share = TICK_BUDGET.saturating_sub(spent) / left as u32;
            left -= 1;
            if names.is_some() || cables_changed || !board.cables_known {
                board.sync_cables(alu);
            }
            if let Some(names) = &names {
                board.sync_chips(alu);
                board.side_names = SIDES
                    .iter()
                    .filter_map(|&side| {
                        Some((side.to_string(), cables::side_port_name(alu, side, names)?))
                    })
                    .collect();
            }
            board.tick(alu, std::time::Instant::now() + share);
        }
    }
    if n % SEND_EVERY == 0 {
        send_views();
    }
}

pub(super) fn send_views() {
    let boards = BOARDS.lock();
    let mut watchers = WATCHERS.lock();
    for (player, (alu, last)) in watchers.iter_mut() {
        let Some(board) = boards.get(alu) else {
            continue;
        };
        let view = board.view(alu);
        if last.as_ref() != Some(&view) {
            let bytes = yog_api::yog_data::to_bytes(&view);
            yog_api::event::net::send_to_player(player, VIEW_CHANNEL, &bytes);
            *last = Some(view);
        }
    }
}

/// Where it's kept in the world store.
pub(super) const STORE_KEY: &str = "boards";

pub fn load() {
    let saved: Vec<(AluId, Saved)> = crate::saved::load(STORE_KEY);
    let mut boards = BOARDS.lock();
    boards.clear();
    for (alu, saved) in saved {
        let mut board = Board::new(saved.tier);
        board.saved = saved;
        board.sync_chips(&alu);
        boards.insert(alu, board);
    }
}

pub fn save() {
    let saved: Vec<(AluId, Saved)> = BOARDS
        .lock()
        .iter()
        .map(|(k, b)| (k.clone(), b.saved.clone()))
        .collect();
    crate::saved::save(STORE_KEY, &saved);
}

pub fn setup(registry: &mut Registry) {
    // Opening an ALU: it gets a board (if new) and the player watches it.
    registry.on::<yog_api::event::block::Use, _>(|e, cx| -> bool {
        let phase = cx.phase();
        let Some(tier) = Tier::from_alu(&e.block_id) else {
            return true;
        };
        if phase != EventPhase::Pre {
            return true;
        }
        let alu = AluId {
            dimension: e.dimension.clone(),
            x: e.pos.x,
            y: e.pos.y,
            z: e.pos.z,
        };
        {
            let mut boards = BOARDS.lock();
            let board = boards
                .entry(alu.clone())
                .or_insert_with(|| Board::new(tier));
            board.saved.tier = tier;
            board.sync_chips(&alu);
        }
        WATCHERS.lock().insert(e.player_name.clone(), (alu, None));
        true // its inventory opens
    });

    registry.on::<yog_api::event::block::Break, _>(|e, cx| {
        let phase = cx.phase();
        if phase == EventPhase::Post && Tier::from_alu(&e.block_id).is_some() {
            let alu = AluId {
                dimension: e.dimension.clone(),
                x: e.pos.x,
                y: e.pos.y,
                z: e.pos.z,
            };
            BOARDS.lock().remove(&alu);
        }
        true
    });

    registry.on_packet(REQUEST_CHANNEL, |ev| {
        let Ok(request) = yog_api::yog_data::read_bytes::<Request>(&ev.payload) else {
            return;
        };
        let alu = request.alu().clone();
        let result = BOARDS.lock().get_mut(&alu).map(|b| b.apply(request));
        if let Some(Err(e)) = result {
            yog_api::event::chat::actionbar(&ev.player, &e);
        }
        if let Some((_, last)) = WATCHERS.lock().get_mut(&ev.player) {
            *last = None;
        }
    });
}
