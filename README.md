<div align="center">

<a id="top"></a>

# 🔲 Yog VLSI

### Design, fabricate and deploy redstone microchips — simulated in Rust

[![Yog Mod Loader](https://img.shields.io/badge/Yog-Mod%20Loader-8a5cf6.svg)](https://github.com/F000NKKK/Yog-Mod-Loader)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.3.0-informational.svg)](yog.toml)
[![yog-api 0.93](https://img.shields.io/badge/yog--api-0.93-orange.svg)](https://crates.io/crates/yog-api)

![Fabric](https://img.shields.io/badge/Fabric-1.20.1%20%7C%201.21.1-success)
![Forge](https://img.shields.io/badge/Forge-1.20.1%20%7C%201.21.1-success)
![NeoForge](https://img.shields.io/badge/NeoForge-1.20.1%20%7C%201.21.1-success)

🇺🇸 **English** | 🇷🇺 [Русский](README.ru.md)

[Overview](#-overview) • [Features](#-features) • [Screens](#-screens) • [Chip tiers](#-chip-tiers) • [Building](#-building) • [Status](#-status)

</div>

---

## 📖 Overview

**Yog VLSI** (*Very Large Scale Integration*) turns redstone circuits into
items. You design a circuit in a private editing world, fabricate it onto a
microchip at the VLSI Workbench, and put chips into an ALU block, where they
run inside a native Rust redstone VM — up to twice vanilla's speed — and
talk to the world through named redstone ports.

The mod runs on the [Yog Mod Loader](https://github.com/F000NKKK/Yog-Mod-Loader):
one `.yog` file for Fabric, Forge and NeoForge, Minecraft 1.20.1 and 1.21.1.

## 🌟 Features

### 🛠️ VLSI Workbench

- **A design library per player**: designs are saved on the server and
  reachable from any workbench.
- **An editing world**: open a design and you are put into a flat, private
  dimension sized by the chip's tier. Build any redstone you like; the field
  is cleared when you save or leave, and nobody stays behind in it.
- **Ports**: place `yog-vlsi:port` blocks anywhere in the circuit and name
  them; each is an input, an output or both.
- **Fabrication**: printing a chip spends resources from the workbench's
  refillable store — about a quarter of what the blocks would cost.

### 🔌 ALU

- **Chips in slots**: an ALU holds several chips and runs them every tick.
- **A node graph**: wire one chip's output port to another's input in the
  ALU's screen; the graph is drawn and edited right there.
- **Named redstone ports** on the ALU's sides connect the graph to the world.

### ⚡ The Redstone VM

- **Vanilla's rules**: dust shapes and power levels, torches, repeaters,
  comparators (with container reading), observers, pistons with slime and
  honey, hoppers, droppers and containers behave as in the game.
- **Fast**: wire shapes are worked out once a step; a full Netherite layer
  steps in about 6 ms (`cargo bench`).

## 🖼️ Screens

Drawn by [Yog-IDLE](https://github.com/F000NKKK/Yog-IDLE)'s screen designer
with the game's font and textures — the same yog-ui code the game runs.

<div align="center">

| | |
|:---:|:---:|
| ![VLSI Workbench](docs/images/screen-workbench.png) | ![ALU](docs/images/screen-alu.png) |
| The workbench: chip, resources, design and cost | The ALU: node graph, chip slots, inventory |
| ![Port](docs/images/screen-port.png) | |
| A port: its name and direction | |

</div>

## 📊 Chip tiers

| Tier | Ticks/s | Editing world | vs vanilla |
|---|---:|---|---:|
| Wood | 5 | 16×16×16 | 0.25× |
| Stone | 10 | 32×32×32 | 0.5× |
| Gold | 20 | 64×64×64 | 1× |
| Iron | 25 | 64×64×64 | 1.25× |
| Diamond | 30 | 128×128×128 | 1.5× |
| Netherite | 40 | 256×256×256 | 2× |

## 🔨 Building

```bash
yog build                           # every platform
YOG_TARGETS=linux-x86_64 yog build  # one platform, faster
```

Produces `artifacts/yog-vlsi.yog` — put it into `<game dir>/yog-mods/`
next to the Yog loader's jar in `mods/`.

## 🚧 Status

Playable and in active development. Next: digital (bus) channels between
ALUs, blueprints as items for sharing designs. See [DESIGN.md](DESIGN.md)
(Russian) for the full design.

## 📄 License

AGPL-3.0-only — see [LICENSE](LICENSE).

<div align="center">

[⬆ Back to top](#top)

</div>
