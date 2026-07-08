# Yog VLSI

**Very Large Scale Integration** — a Minecraft mod for [Yog Mod Loader](https://github.com/F000NKKK/Yog-Mod-Loader) that lets you design, fabricate, and deploy redstone microchips with a Rust-accelerated simulation VM.

## Concept

- **Crafting Workbench** — a special table where you place microchips and edit their internal redstone circuits in a virtual creative world.
- **Microchip Tiers** — Wood (5 ticks/s), Stone (10), Gold (20), Iron (25), Diamond (30), Netherite (40 ticks/s). Higher tier = larger virtual world + faster simulation. Netherite runs at 2× vanilla speed.
- **Virtual Redstone World** — enter a creative-mode instance (16×16 to 256×256 depending on tier) to design redstone circuits. Connect to I/O ports on the chip boundary. All entities (pearls, etc.) are purged — pure redstone logic.
- **ALU Block** — place finished microchips into an ALU. Passthrough mode (1:1 redstone signal to a side) or link multiple chips internally as a node graph — connect an output port of one chip to an input port of another.
- **I/O Nodes** — configurable in 3 modes (input / output / bidirectional) on each side of the ALU.
- **Rust VM** — redstone simulation runs in a custom Rust virtual machine for extreme performance. Hoppers, chests, and all standard redstone blocks work as expected.
- **Resource Ammo** — crafting chips consumes ~25% of vanilla recipe resources per block (smaller scale). Resources are loaded like MFU paint — no storage limit, just refill.
- **Server Storage** — chip designs (port list, circuit data) are persisted on the server.

## Requirements

Built against [Yog Mod Loader](https://github.com/F000NKKK/Yog-Mod-Loader) 0.2.0+.

## Building

```bash
yog build
```

Produces `artifacts/yog-vlsi.yog` — drop it into `<game dir>/yog-mods/`.

## License

AGPL-3.0-only — see [LICENSE](LICENSE).
