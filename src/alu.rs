//! ALU block: accepts microchips, routes signals, links chips into node graphs.

use yog_api::Registry;

pub fn register(_registry: &mut Registry) {
    // TODO: register ALU block + item
    // TODO: I/O node configuration (3 modes per side: input / output / bidirectional)
    // TODO: passthrough mode (1:1 redstone signal to a block face)
    // TODO: internal chip linking (node graph: connect output ports to input ports)
    // TODO: tick handler → step the internal chip VM(s)
}
