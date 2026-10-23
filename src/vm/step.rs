//! Settling and stepping: a step is one redstone tick; settling steps
//! until nothing changes, skipping the ticks only timers count down.

use super::{BlockType, RedstoneVM};

impl RedstoneVM {
    /// A new server tick: every nested ALU's chips get their tier's steps
    /// again (all the way down).
    pub fn new_tick(&mut self) {
        for alu in self.nested.values_mut() {
            for c in &mut alu.chips {
                c.left = c.budget;
                c.sim.new_tick();
            }
        }
    }

    /// Steps until the circuit settles — a step changes nothing — or for
    /// `budget` steps; how many it took. A circuit answers its inputs in
    /// the steps it needs, not one a server tick.
    pub fn settle(&mut self, budget: u32) -> u32 {
        self.settle_with(budget, None)
    }

    /// [`settle`](Self::settle), stopping early at `deadline` (looked at
    /// every 32 steps) — the server's tick comes first; the circuit goes on
    /// next tick, as if out of steps.
    pub fn settle_until(&mut self, budget: u32, deadline: std::time::Instant) -> u32 {
        let mut done = 0;
        while done < budget {
            let chunk = (budget - done).min(32);
            let n = self.settle(chunk);
            done += n;
            if n < chunk || std::time::Instant::now() >= deadline {
                break;
            }
        }
        done
    }

    /// [`settle`](Self::settle), `after` called after each step.
    pub(crate) fn settle_traced(&mut self, budget: u32, after: &mut dyn FnMut(&RedstoneVM)) -> u32 {
        self.settle_with(budget, Some(after))
    }

    /// Settling; `after` (when there is one) is called after every step, so
    /// no step is skipped — else steps that would only count timers down
    /// are passed over in one go.
    pub(super) fn settle_with(
        &mut self,
        budget: u32,
        mut after: Option<&mut dyn FnMut(&RedstoneVM)>,
    ) -> u32 {
        // Settled and its inputs as they were: the step would change nothing.
        if self.quiet {
            return 0;
        }
        let (mut before, mut before_untimed) = self.state_parts();
        let mut n = 0;
        while n < budget {
            n += 1;
            self.step();
            if let Some(f) = after.as_mut() {
                f(self);
            }
            let (now, untimed) = self.state_parts();
            if now == before {
                // Not while a nested ALU's chip waits for its next tick's steps.
                self.quiet = self.nested.values().all(|a| a.stable);
                return n;
            }
            // Only timers moved, and nothing waits to run: the next steps
            // till a timer runs out do the same — count them down at once.
            before = now;
            if after.is_none() && untimed == before_untimed && self.idle_but_timers() {
                let k = self.min_timer().saturating_sub(1).min(budget - n);
                if k > 0 {
                    self.count_down(k);
                    n += k;
                    before = self.state();
                }
            }
            before_untimed = untimed;
        }
        budget
    }

    /// Nothing but timers would move: no ALU, reader or channel waiting.
    pub(super) fn idle_but_timers(&self) -> bool {
        self.dirty.is_empty()
            && self.unstable.is_empty()
            && self.dirty_readers.is_empty()
            && self.nested.values().all(|a| a.stable)
    }

    /// The smallest running timer (0: none).
    pub(super) fn min_timer(&self) -> u32 {
        self.components
            .iter()
            .chain(&self.movers)
            .map(|&i| u32::from(self.get(i).timer))
            .filter(|&t| t > 0)
            .min()
            .unwrap_or(0)
    }

    /// `k` steps of timers only: each running timer `k` lower, the clock `k` on.
    pub(super) fn count_down(&mut self, k: u32) {
        let cells: Vec<u32> = self
            .components
            .iter()
            .chain(&self.movers)
            .copied()
            .collect();
        for i in cells {
            let c = self.get_mut(i);
            if c.timer > 0 {
                c.timer -= k as u8;
            }
        }
        self.tick += u64::from(k);
    }

    /// Everything that decides what it does next: its cells, whether it's
    /// settled, what its input ports get. Two VMs of one circuit with the
    /// same fingerprint go on alike.
    pub(crate) fn fingerprint(&self, inputs: &[(u32, u32, u32)]) -> u64 {
        let mut h = self.state() ^ u64::from(self.quiet) << 63;
        for &(x, y, z) in inputs {
            let v = if self.in_bounds(x as i32, y as i32, z as i32) {
                self.get(self.idx(x, y, z)).port_in
            } else {
                0
            };
            h = (h ^ u64::from(v)).wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// Whether a table can stand for it ([`crate::compile`]): only redstone
    /// whose next step is its cells' business — no items or randomness, no
    /// observers (they count steps), no pistons, no ALUs inside.
    pub(crate) fn tabulable(&self) -> Result<(), &'static str> {
        if !self.nested.is_empty() {
            return Err("it holds ALUs");
        }
        for &i in &self.active {
            match self.get(i).block {
                BlockType::Observer { .. } => return Err("an observer"),
                BlockType::Piston { .. }
                | BlockType::PistonHead { .. }
                | BlockType::Slime
                | BlockType::Honey => return Err("pistons"),
                BlockType::Container { .. }
                | BlockType::Hopper { .. }
                | BlockType::Dropper { .. } => return Err("items"),
                BlockType::Button { .. } => return Err("a button"),
                BlockType::Alu | BlockType::Bus => return Err("it holds ALUs"),
                _ => {}
            }
        }
        Ok(())
    }

    /// A fingerprint of every cell's state (block, levels, timers).
    pub(super) fn state(&self) -> u64 {
        self.state_parts().0
    }

    /// [`state`](Self::state), and the same without the timers.
    pub(super) fn state_parts(&self) -> (u64, u64) {
        let mut u: u64 = 0xcbf2_9ce4_8422_2325;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        mix(self.alu_changes);
        for &i in &self.logic {
            let c = self.get(i);
            let code = c.block.code();
            mix(code);
            let untimed = u64::from(c.power)
                | u64::from(c.strong) << 8
                | u64::from(c.weak) << 16
                | u64::from(i) << 32;
            mix(untimed | u64::from(c.timer) << 24);
            mix(u64::from(c.watched));
            u = (u ^ untimed ^ u64::from(c.watched) << 40 ^ code.rotate_left(17))
                .wrapping_mul(0x0100_0000_01b3);
        }
        // A nested ALU still running (a clock in a chip, a chip out of
        // steps): the circuit isn't settled.
        if self.nested.values().any(|a| !a.stable) {
            mix(self.tick);
        }
        (h, u ^ self.alu_changes)
    }

    pub fn step(&mut self) {
        self.tick += 1;
        self.shape_wire();
        self.strong_power();
        self.settle_wire();
        self.wire_power();
        self.read_power();
        self.settle_components();
        self.move_items();
    }
}
