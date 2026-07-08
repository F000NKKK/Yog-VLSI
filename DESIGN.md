# Yog VLSI — Design Document

> **Very Large Scale Integration** — проектирование, fabrication и деплой редстоун-микросхем с Rust-ускоренной симуляцией.

## 1. Обзор

Мод добавляет три ключевых блока: **VLSI Workbench** (верстак), **Microchip** (микросхема, 6 тиров) и **ALU** (арифметико-логическое устройство). Игрок проектирует редстоун-схему в виртуальном мире, «печатает» её на микросхему, вставляет микросхему в ALU — и получает сверхбыструю симуляцию своей логики в реальном мире.

Вся симуляция редстоуна выполняется нативным Rust VM — это даёт скорость до 40 тиков/с (2× vanilla) на Netherite-чипе.

## 2. Microchip (микросхема)

### 2.1 Тиры

| Тир | Материал | Тиков/с | Размер мира | Кратно vanilla |
|-----|----------|---------|-------------|----------------|
| Wood | Дерево | 5 | 16×16×16 | 0.25× |
| Stone | Камень | 10 | 32×32×32 | 0.5× |
| Gold | Золото | 20 | 64×64×64 | 1× |
| Iron | Железо | 25 | 64×64×64 | 1.25× |
| Diamond | Алмаз | 30 | 128×128×128 | 1.5× |
| Netherite | Незерит | 40 | 256×256×256 | 2× |

- Чем выше тир — тем больше виртуальный мир и быстрее симуляция.
- Каждый чип имеет уникальный UUID.
- Чип хранит метаданные в NBT (ChipMeta): ID, тир, имя, список портов.
- Схема (CircuitData) хранится на сервере через Storage API (отдельно от NBT, т.к. для чипа 256×256 это десятки тысяч блоков).

### 2.2 Порты

На границе виртуального мира располагаются I/O-порты — специальные блоки, через которые чип взаимодействует с внешним миром (ALU).

```
Порт {
    label: "A0" | "CLK" | "Q" | ...
    side:  North | South | East | West
    index: 0..size-1  (позиция вдоль границы)
    dir:   Input | Output | Bidirectional
}
```

Порты — это блоки `yog-vlsi:port` с NBT `{mode: "input"|"output"|"bidirectional"}`. В виртуальном мире они размещаются на границе (x=0, x=size-1, z=0, z=size-1).

## 3. VLSI Workbench (верстак)

### 3.1 Крафт

```
I S I
S C S
I R I

I = iron_ingot
S = smooth_stone
C = crafting_table
R = redstone_block
```

### 3.2 Ресурсная система (Resource Ammo)

Верстак хранит ресурсы по принципу «краски в MFU»:
- Нет лимита на хранение.
- Игрок пополняет ресурсы, кидая предметы в верстак.
- Ресурсы трекаются как `HashMap<item_id, quantity>`.
- Персистентность: Storage API, ключ = позиция блока.

### 3.3 Fabrication (печать чипа)

Стоимость печати = **25% от vanilla-рецепта** каждого блока в схеме (размеры маленькие).

Пример:
- Redstone wire: 1 redstone dust → 0.25 (округляется вверх)
- Repeater: 2 torches + 1 dust + 3 stone → 0.5 dust + 0.5 stick + 0.75 stone за блок
- И т.д.

Ресурсы списываются из хранилища верстака, чип программируется (NBT + CircuitData на сервер).

### 3.4 Virtual World Editor (проектирование)

По нажатию кнопки «Design» в GUI верстака игрок телепортируется в creative-мир размером с тир чипа:
- Плоский мир из bedrock-пола.
- На границах — блоки портов (yog-vlsi:port), которые можно настраивать.
- Игрок строит редстоун-схему из любых доступных блоков.
- Все энтити (pearls, item frames, etc.) запрещены и чистятся при входе/выходе.
- Кнопка «Save» → CircuitData сохраняется на сервер, игрок возвращается.

## 4. ALU (арифметико-логическое устройство)

### 4.1 Крафт

```
G C G
C R C
G D G

G = gold_ingot
C = copper_ingot
R = repeater
D = diamond
```

### 4.2 Режимы работы

#### Passthrough (1:1)
Один чип. Каждый порт чипа маппится на сторону блока ALU 1-в-1.
- Input-порт чипа ← redstone signal с соответствующей стороны ALU
- Output-порт чипа → redstone signal на соответствующую сторону ALU

#### Internal Graph (chip-to-chip)
Несколько чипов внутри одного ALU. Нодовый редактор:
- **I/O Nodes** (слева/справа в GUI): входы и выходы ALU во внешний мир. Каждый переключается в 3 режимах: Input, Output, Bidirectional.
- **Внутренние связи (линки)**: выходной порт чипа A → входной порт чипа B → входной порт чипа C → выходной порт ALU.
- По сути — визуальный нодовый граф, где узлы = порты чипов + I/O-ноды ALU, рёбра = соединения.

### 4.3 Tick-обработчик

Каждый серверный тик (20/с):
- Для каждого чипа в ALU — шаг VM.
- Частота шага = `tick_rate` чипа. Netherite (40) → 2 шага VM за 1 серверный тик. Wood (5) → 1 шаг VM за 4 серверных тика.
- После шага: чтение output-портов → обновление redstone-сигнала ALU.

### 4.4 Установка чипов

Через GUI (нодовый редактор), не через ПКМ. Игрок открывает ALU, видит список доступных чипов в инвентаре, перетаскивает их в слоты ALU.

## 5. Rust Redstone VM

### 5.1 Архитектура

```
RedstoneVM {
    tier: Tier,
    width, height: u32,
    grid: Vec<Cell>,          // width × width × height
    updates: VecDeque<Pos>,   // очередь обновлений
    tick: u64,
}
```

Каждая ячейка:
```
Cell {
    block: BlockType,
    power: 0..15,
    strongly_powered: bool,
    weakly_powered: bool,
    repeater_timer: u8,
    torch_burnout: u8,
}
```

### 5.2 Алгоритм тика

1. **Reset** — сброс power у всех проводов, сброс weakly_powered.
2. **Process updates** — обработка очереди: факелы (инверсия), повторители (задержка), observer (2-тиковый импульс).
3. **Propagation (BFS)** — от всех источников питания (факелы, повторители, рычаги, кнопки, redstone_block, target, pressure plates, trapped_chest, detector_rail, I/O-порты) → BFS по redstone_wire с затуханием -1 за блок.
4. **Solid powering** — strong power от редстоун-компонентов в solid-блоки; weak power через соседние solid-блоки.
5. **Timers** — decrement repeater_timer, torch_burnout.

### 5.3 Поддерживаемые блоки

**Редстоун:** wire, torch, wall_torch, repeater, comparator, lever, stone_button, wood_button, stone_pressure_plate, wood_pressure_plate, light_weighted_pressure_plate, heavy_weighted_pressure_plate, observer, note_block, target, lamp, redstone_block.

**Solid/проводящие:** Solid (все обычные блоки), Glass (непроводящий), piston, sticky_piston.

**Контейнеры:** chest, trapped_chest, ender_chest, shulker_box, barrel, hopper, dropper, dispenser, furnace, blast_furnace, smoker, brewing_stand.
- Инвентари не симулируются, но блоки корректно проводят/блокируют редстоун.
- Trapped chest выдаёт power = количеству «зрителей» (упрощённо).

**Механика:** slime_block, honey_block, tnt, iron_door, wood_door, iron_trapdoor, wood_trapdoor, fence_gate, rail, powered_rail, detector_rail, activator_rail.

### 5.4 План развития VM

- **Comparator logic** — compare/subtract режимы, чтение контейнеров.
- **Observer detection** — детект block updates в VM.
- **Piston push/pull** — физика движения блоков.
- **Hopper/Dropper item transfer** — симуляция предметов.
- **Noteblock** — частота от питания.

## 6. Команды (debug/тестирование)

| Команда | Описание |
|---------|----------|
| `/vlsi` | Справка |
| `/vlsi chip <tier>` | Выдать чистый чип |
| `/vlsi info` | Показать NBT чипа в руке |
| `/vlsi test <tier>` | Создать тестовый чип (redstone_block → wire → lamp + порты) |
| `/vlsi vm step` | Шаг симуляции на чипе в руке |

## 7. Лицензия

AGPL-3.0-only — весь код мода. Серверное хранение схем покрывается AGPL-сетевым пунктом.

## 8. Репозиторий

https://github.com/F000NKKK/Yog-VLSI

Зависит от [Yog Mod Loader](https://github.com/F000NKKK/Yog-Mod-Loader) 0.2.0+.
