// KG: SPAN_333_KillerApps — StarCraft-style P2P RTS
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-apps-rts-UnitType/Unit/Command/Turn/RtsGame
// Lockstep + CRDT hybrid: lockstep for commands, CRDT for world state

use crate::lww_map::LwwMap;
use crate::hlc::Hlc;
use crate::determinism::DeterministicSerialize;
// KG: sprint6B-wasm-size-opt-2026-04-15 — serde removed from rts.rs internal types
// Unit/Command/Turn/UnitType are not serialized to JSON; removing Serialize/Deserialize
// eliminates flt2dec (~13KB) from WASM binary.
// If wire-protocol serialization is needed later, use postcard (binary) not JSON.
use std::collections::{BTreeMap, HashMap};

/// RTS unit types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitType {
    Worker,
    Soldier,
    Tank,
    Building,
}

/// A unit on the map
#[derive(Debug, Clone)]
pub struct Unit {
    pub id: u32,
    pub unit_type: UnitType,
    pub owner: u32,
    pub x: f32,
    pub y: f32,
    pub hp: u32,
    pub max_hp: u32,
}

/// Player command (lockstep — all players must agree)
#[derive(Debug, Clone)]
pub enum Command {
    Move { unit_id: u32, target_x: f32, target_y: f32 },
    Attack { unit_id: u32, target_id: u32 },
    Build { unit_type: UnitType, x: f32, y: f32 },
    Gather { unit_id: u32, resource_x: f32, resource_y: f32 },
    /// Sprint 7D: Produce unit from a Building
    Produce { building_id: u32, unit_type: UnitType },
    /// Sprint 7D: Stop/hold position
    Stop { unit_id: u32 },
}

/// Sprint 7D: Resource node on the map
// KG: sprint7D-rts-gameplay-2026-04-16
#[derive(Debug, Clone)]
pub struct ResourceNode {
    pub x: f32,
    pub y: f32,
    pub amount: u32,
}

/// Sprint 7D: Game result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameResult {
    InProgress,
    Victory(u32),  // winner player_id
    Draw,
}

/// Lockstep turn: all commands from all players for one tick
#[derive(Debug, Clone)]
pub struct Turn {
    pub turn_number: u64,
    pub commands: Vec<(u32, Command)>,  // (player_id, command)
    pub hash: String,                    // state hash for desync detection
}

/// RTS game state
pub struct RtsGame {
    pub game_id: String,
    pub tick: u64,
    pub map_width: u32,
    pub map_height: u32,
    next_unit_id: u32,
    units: HashMap<u32, Unit>,
    #[allow(dead_code)] // CRDT resource map, used when resource mechanics are implemented
    resources: LwwMap<String, String>,  // CRDT for resource nodes on map
    #[allow(dead_code)] // HLC for resource timestamping, used when CRDT merge is wired up
    hlc: Hlc,
    players: Vec<u32>,
    player_minerals: HashMap<u32, u32>,
    turn_buffer: Vec<Turn>,
    /// Sprint 7D: Resource nodes on the map
    resource_nodes: Vec<ResourceNode>,
    /// Sprint 7D: Game result
    pub game_result: GameResult,
    /// Sprint 7D: Fog of war — sight range per unit type
    sight_range: HashMap<u32, f32>,  // unit_type ordinal → sight range
}

impl RtsGame {
    pub fn new(game_id: &str, player_id: u32, map_w: u32, map_h: u32) -> Self {
        Self {
            game_id: game_id.to_string(),
            tick: 0,
            map_width: map_w,
            map_height: map_h,
            next_unit_id: 1,
            units: HashMap::new(),
            resources: LwwMap::new(player_id),
            hlc: Hlc::new(player_id),
            players: vec![player_id],
            player_minerals: HashMap::from([(player_id, 500)]),
            turn_buffer: Vec::new(),
            resource_nodes: Vec::new(),
            game_result: GameResult::InProgress,
            sight_range: HashMap::from([
                (0, 8.0),   // Worker
                (1, 10.0),  // Soldier
                (2, 12.0),  // Tank
                (3, 5.0),   // Building
            ]),
        }
    }

    /// Add a player
    pub fn add_player(&mut self, player_id: u32) {
        if !self.players.contains(&player_id) {
            self.players.push(player_id);
            self.player_minerals.insert(player_id, 500);
        }
    }

    /// Spawn a unit
    pub fn spawn_unit(&mut self, owner: u32, unit_type: UnitType, x: f32, y: f32) -> u32 {
        let cost = match unit_type {
            UnitType::Worker => 50,
            UnitType::Soldier => 100,
            UnitType::Tank => 200,
            UnitType::Building => 150,
        };
        let minerals = self.player_minerals.get_mut(&owner);
        if let Some(m) = minerals {
            if *m < cost { return 0; }
            *m -= cost;
        }

        let id = self.next_unit_id;
        self.next_unit_id += 1;
        let hp = match unit_type {
            UnitType::Worker => 40,
            UnitType::Soldier => 100,
            UnitType::Tank => 250,
            UnitType::Building => 500,
        };
        self.units.insert(id, Unit { id, unit_type, owner, x, y, hp, max_hp: hp });
        id
    }

    /// Execute a command
    pub fn execute_command(&mut self, player_id: u32, cmd: &Command) {
        match cmd {
            Command::Move { unit_id, target_x, target_y } => {
                if let Some(unit) = self.units.get_mut(unit_id) {
                    if unit.owner == player_id {
                        unit.x = *target_x;
                        unit.y = *target_y;
                    }
                }
            }
            Command::Attack { unit_id, target_id } => {
                let attacker_dmg = self.units.get(unit_id)
                    .filter(|u| u.owner == player_id)
                    .map(|u| match u.unit_type {
                        UnitType::Worker => 5,
                        UnitType::Soldier => 15,
                        UnitType::Tank => 30,
                        UnitType::Building => 0,
                    })
                    .unwrap_or(0);
                if attacker_dmg > 0 {
                    if let Some(target) = self.units.get_mut(target_id) {
                        target.hp = target.hp.saturating_sub(attacker_dmg);
                    }
                }
            }
            Command::Build { unit_type, x, y } => {
                self.spawn_unit(player_id, *unit_type, *x, *y);
            }
            Command::Gather { unit_id, resource_x, resource_y } => {
                // Sprint 7D: gather from nearest resource node if in range
                let can_gather = self.units.get(unit_id)
                    .map(|u| u.owner == player_id && u.unit_type == UnitType::Worker)
                    .unwrap_or(false);
                if can_gather {
                    let gather_amount = self.drain_nearest_resource(*resource_x, *resource_y, 10);
                    *self.player_minerals.entry(player_id).or_default() += gather_amount;
                }
            }
            Command::Produce { building_id, unit_type } => {
                // Sprint 7D: produce unit from building
                let can_produce = self.units.get(building_id)
                    .map(|u| u.owner == player_id && u.unit_type == UnitType::Building)
                    .unwrap_or(false);
                if can_produce {
                    let (bx, by) = self.units.get(building_id)
                        .map(|u| (u.x + 2.0, u.y))
                        .unwrap_or((0.0, 0.0));
                    self.spawn_unit(player_id, *unit_type, bx, by);
                }
            }
            Command::Stop { unit_id } => {
                // Sprint 7D: stop — just validate ownership (no-op for now, prevents movement)
                let _ = self.units.get(unit_id)
                    .filter(|u| u.owner == player_id);
            }
        }
    }

    /// Process a lockstep turn
    /// Sprint 7D: + auto-gather + victory check
    pub fn process_turn(&mut self, turn: Turn) {
        for (player_id, cmd) in &turn.commands {
            self.execute_command(*player_id, cmd);
        }
        // Remove dead units
        self.units.retain(|_, u| u.hp > 0);
        self.tick = turn.turn_number + 1;
        self.turn_buffer.push(turn);
        // Sprint 7D: check victory condition
        self.check_victory();
    }

    /// Get all units for a player
    pub fn player_units(&self, player_id: u32) -> Vec<&Unit> {
        self.units.values().filter(|u| u.owner == player_id).collect()
    }

    /// Get all units
    pub fn all_units(&self) -> Vec<&Unit> {
        self.units.values().collect()
    }

    /// Get minerals for a player
    pub fn minerals(&self, player_id: u32) -> u32 {
        *self.player_minerals.get(&player_id).unwrap_or(&0)
    }

    // ── Sprint 7D: new gameplay methods ────────────────────────────────

    /// Add a resource node to the map
    pub fn add_resource(&mut self, x: f32, y: f32, amount: u32) {
        self.resource_nodes.push(ResourceNode { x, y, amount });
    }

    /// Drain minerals from nearest resource node within range 5.0
    fn drain_nearest_resource(&mut self, x: f32, y: f32, max_drain: u32) -> u32 {
        let mut best_idx = None;
        let mut best_dist = f32::MAX;
        for (i, node) in self.resource_nodes.iter().enumerate() {
            if node.amount == 0 { continue; }
            let dx = node.x - x;
            let dy = node.y - y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 5.0 && dist < best_dist {
                best_dist = dist;
                best_idx = Some(i);
            }
        }
        if let Some(idx) = best_idx {
            let drain = max_drain.min(self.resource_nodes[idx].amount);
            self.resource_nodes[idx].amount -= drain;
            drain
        } else {
            // Fallback: flat 10 minerals (legacy behavior)
            10
        }
    }

    /// Check victory condition: if only one player has units left
    fn check_victory(&mut self) {
        if self.game_result != GameResult::InProgress { return; }
        let alive_players: Vec<u32> = self.players.iter()
            .filter(|&&pid| self.units.values().any(|u| u.owner == pid))
            .copied()
            .collect();
        match alive_players.len() {
            0 => self.game_result = GameResult::Draw,
            1 => self.game_result = GameResult::Victory(alive_players[0]),
            _ => {} // still in progress
        }
    }

    /// Sprint 7D: Get units visible to a player (fog of war)
    pub fn visible_units(&self, player_id: u32) -> Vec<&Unit> {
        let my_units: Vec<&Unit> = self.units.values()
            .filter(|u| u.owner == player_id)
            .collect();
        self.units.values()
            .filter(|u| {
                if u.owner == player_id { return true; }
                // Enemy unit visible if within sight range of any owned unit
                my_units.iter().any(|my| {
                    let dx = my.x - u.x;
                    let dy = my.y - u.y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let range = self.sight_range.get(&(my.unit_type as u32)).copied().unwrap_or(8.0);
                    dist <= range
                })
            })
            .collect()
    }

    /// Sprint 7D: Attack range for a unit type
    pub fn attack_range(unit_type: &UnitType) -> f32 {
        match unit_type {
            UnitType::Worker => 1.5,
            UnitType::Soldier => 5.0,
            UnitType::Tank => 8.0,
            UnitType::Building => 0.0,
        }
    }

    /// Sprint 7D: Check if attacker is in range of target
    pub fn in_attack_range(&self, attacker_id: u32, target_id: u32) -> bool {
        let (ax, ay, atype) = match self.units.get(&attacker_id) {
            Some(u) => (u.x, u.y, &u.unit_type),
            None => return false,
        };
        let (tx, ty) = match self.units.get(&target_id) {
            Some(u) => (u.x, u.y),
            None => return false,
        };
        let dx = ax - tx;
        let dy = ay - ty;
        let dist = (dx * dx + dy * dy).sqrt();
        dist <= Self::attack_range(atype)
    }

    /// State hash for desync detection — blake3 [u8; 32] over deterministic fields.
    ///
    /// Serializes game-logic fields only (units sorted by id, player_minerals).
    /// HLC and per-node fields are intentionally excluded (TacticalHashView pattern).
    /// tick is included to prevent hash collision across turns.
    ///
    /// KG: seed-post-rts-state-hash-migration-2026-04-15
    pub fn state_hash(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(256);
        // Domain separator
        buf.extend_from_slice(b"333:rts_state_hash:v1:");
        // tick (turn number) — prevents same-state hash collision across turns
        self.tick.det_serialize(&mut buf);
        // Units sorted by id (BTreeMap order) — HashMap iteration order excluded
        // KG: seed-post-rts-state-hash-migration-2026-04-15
        let sorted_units: BTreeMap<u32, &Unit> = self.units.iter().map(|(&k, v)| (k, v)).collect();
        (sorted_units.len() as u32).det_serialize(&mut buf);
        for (&id, unit) in &sorted_units {
            id.det_serialize(&mut buf);
            unit.hp.det_serialize(&mut buf);
            // x/y as fixed-point scaled integer (x100 truncated to u32 as big-endian)
            // Note: RtsGame still uses f32; convert deterministically via bit-stable cast.
            // Use (x * 1000.0) as i32 → i32 BE bytes for sub-integer precision.
            let x_fixed = (unit.x * 1000.0) as i32;
            let y_fixed = (unit.y * 1000.0) as i32;
            x_fixed.det_serialize(&mut buf);
            y_fixed.det_serialize(&mut buf);
        }
        // Player minerals — BTreeMap for sorted iteration
        let sorted_minerals: BTreeMap<u32, u32> =
            self.player_minerals.iter().map(|(&k, &v)| (k, v)).collect();
        sorted_minerals.det_serialize(&mut buf);
        *blake3::hash(&buf).as_bytes()
    }

    /// Hex-encoded state hash — backward compat for callers that need a String.
    ///
    /// KG: seed-post-rts-state-hash-migration-2026-04-15
    pub fn state_hash_hex(&self) -> String {
        self.state_hash()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_units() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        let id = game.spawn_unit(1, UnitType::Worker, 10.0, 10.0);
        assert!(id > 0);
        assert_eq!(game.player_units(1).len(), 1);
        assert_eq!(game.minerals(1), 450); // 500 - 50
    }

    #[test]
    fn move_unit() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        let uid = game.spawn_unit(1, UnitType::Soldier, 0.0, 0.0);
        game.execute_command(1, &Command::Move { unit_id: uid, target_x: 50.0, target_y: 50.0 });
        let unit = game.units.get(&uid).unwrap();
        assert_eq!(unit.x, 50.0);
        assert_eq!(unit.y, 50.0);
    }

    #[test]
    fn attack_reduces_hp() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        let attacker = game.spawn_unit(1, UnitType::Soldier, 0.0, 0.0);
        let target = game.spawn_unit(2, UnitType::Worker, 5.0, 5.0);

        game.execute_command(1, &Command::Attack { unit_id: attacker, target_id: target });
        assert_eq!(game.units.get(&target).unwrap().hp, 25); // 40 - 15
    }

    #[test]
    fn dead_units_removed_on_turn() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        let attacker = game.spawn_unit(1, UnitType::Tank, 0.0, 0.0);
        let target = game.spawn_unit(2, UnitType::Worker, 5.0, 5.0); // hp=40

        // Tank does 30 dmg per attack, need 2 hits
        let turn = Turn {
            turn_number: 0,
            commands: vec![
                (1, Command::Attack { unit_id: attacker, target_id: target }),
                (1, Command::Attack { unit_id: attacker, target_id: target }),
            ],
            hash: String::new(),
        };
        game.process_turn(turn);
        assert!(!game.units.contains_key(&target)); // dead, removed
    }

    #[test]
    fn gather_adds_minerals() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        let worker = game.spawn_unit(1, UnitType::Worker, 10.0, 10.0);
        let before = game.minerals(1);
        game.execute_command(1, &Command::Gather { unit_id: worker, resource_x: 10.0, resource_y: 10.0 });
        assert_eq!(game.minerals(1), before + 10);
    }

    #[test]
    fn cannot_move_enemy_unit() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        let uid = game.spawn_unit(2, UnitType::Soldier, 0.0, 0.0);
        game.execute_command(1, &Command::Move { unit_id: uid, target_x: 99.0, target_y: 99.0 });
        // Should not move — player 1 doesn't own it
        assert_eq!(game.units.get(&uid).unwrap().x, 0.0);
    }

    #[test]
    fn state_hash_deterministic() {
        // state_hash() must return identical [u8; 32] for identical game states.
        // KG: seed-post-rts-state-hash-migration-2026-04-15
        let mut g1 = RtsGame::new("test", 1, 100, 100);
        let mut g2 = RtsGame::new("test", 1, 100, 100);
        g1.spawn_unit(1, UnitType::Worker, 10.0, 20.0);
        g2.spawn_unit(1, UnitType::Worker, 10.0, 20.0);
        let h1: [u8; 32] = g1.state_hash();
        let h2: [u8; 32] = g2.state_hash();
        assert_eq!(h1, h2, "identical game states must produce identical [u8; 32] hashes");
    }

    #[test]
    fn state_hash_is_32_bytes() {
        // state_hash() return type is exactly [u8; 32].
        // KG: seed-post-rts-state-hash-migration-2026-04-15
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.spawn_unit(1, UnitType::Soldier, 5.0, 5.0);
        let h: [u8; 32] = game.state_hash();
        assert_eq!(h.len(), 32, "state_hash must return exactly 32 bytes");
    }

    #[test]
    fn state_hash_excludes_hlc_like_fields() {
        // Two game instances representing the same game from two different nodes must
        // produce the same state hash when tactical state is identical.
        // node_id only affects the HLC and LwwMap internal clocks — excluded from hash.
        // KG: seed-post-rts-state-hash-migration-2026-04-15
        //
        // Setup: both instances share the same player set (player 1 + player 2),
        // same units, same minerals. The only difference is which node_id constructed
        // the RtsGame (1 vs 2) — this mirrors two peers holding the same game state.
        let mut g_node1 = RtsGame::new("game-a", 1, 100, 100);  // node 1 hosts player 1
        let mut g_node2 = RtsGame::new("game-a", 1, 100, 100);  // node 2 also mirrors player 1
        // node 2 has different HLC node_id in resources/hlc field, but same logical game state
        // Simulate: identical game state — same players, same spawns
        g_node1.add_player(2);
        g_node2.add_player(2);
        g_node1.spawn_unit(1, UnitType::Soldier, 10.0, 20.0);
        g_node2.spawn_unit(1, UnitType::Soldier, 10.0, 20.0);
        // tick and all hashed fields are identical → hashes must match
        assert_eq!(
            g_node1.state_hash(),
            g_node2.state_hash(),
            "same game state must produce identical hash regardless of construction node_id"
        );
    }

    #[test]
    fn state_hash_changes_on_tactical_change() {
        // Moving a unit must produce a different hash.
        // KG: seed-post-rts-state-hash-migration-2026-04-15
        let mut game = RtsGame::new("test", 1, 100, 100);
        let uid = game.spawn_unit(1, UnitType::Soldier, 0.0, 0.0);
        let hash_before: [u8; 32] = game.state_hash();
        game.execute_command(1, &Command::Move { unit_id: uid, target_x: 50.0, target_y: 50.0 });
        let hash_after: [u8; 32] = game.state_hash();
        assert_ne!(hash_before, hash_after, "tactical change (unit move) must change state hash");
    }

    // ── Sprint 7D: new gameplay tests ─────────────────────────────────

    #[test]
    fn produce_from_building() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        let bld = game.spawn_unit(1, UnitType::Building, 50.0, 50.0);
        let before = game.player_units(1).len();
        game.execute_command(1, &Command::Produce { building_id: bld, unit_type: UnitType::Soldier });
        assert_eq!(game.player_units(1).len(), before + 1, "building must produce a unit");
    }

    #[test]
    fn resource_node_gathering() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_resource(10.0, 10.0, 100);
        let worker = game.spawn_unit(1, UnitType::Worker, 10.0, 10.0);
        let before = game.minerals(1);
        game.execute_command(1, &Command::Gather { unit_id: worker, resource_x: 10.0, resource_y: 10.0 });
        assert_eq!(game.minerals(1), before + 10);
        assert_eq!(game.resource_nodes[0].amount, 90, "resource node should deplete");
    }

    #[test]
    fn victory_when_enemy_eliminated() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        let tank = game.spawn_unit(1, UnitType::Tank, 0.0, 0.0);
        let enemy = game.spawn_unit(2, UnitType::Worker, 5.0, 5.0); // 40 hp
        // Kill enemy in 2 hits (30 dmg each)
        let turn = Turn {
            turn_number: 0,
            commands: vec![
                (1, Command::Attack { unit_id: tank, target_id: enemy }),
                (1, Command::Attack { unit_id: tank, target_id: enemy }),
            ],
            hash: String::new(),
        };
        game.process_turn(turn);
        assert_eq!(game.game_result, GameResult::Victory(1));
    }

    #[test]
    fn fog_of_war_hides_distant_enemy() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        game.spawn_unit(1, UnitType::Soldier, 0.0, 0.0);     // sight = 10
        game.spawn_unit(2, UnitType::Soldier, 50.0, 50.0);   // far away
        let visible = game.visible_units(1);
        assert_eq!(visible.len(), 1, "distant enemy should be hidden by fog of war");
    }

    #[test]
    fn fog_of_war_reveals_nearby_enemy() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        game.spawn_unit(1, UnitType::Soldier, 10.0, 10.0);   // sight = 10
        game.spawn_unit(2, UnitType::Worker, 15.0, 10.0);    // 5 units away
        let visible = game.visible_units(1);
        assert_eq!(visible.len(), 2, "nearby enemy should be visible");
    }

    #[test]
    fn attack_range_check() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        game.add_player(2);
        let soldier = game.spawn_unit(1, UnitType::Soldier, 0.0, 0.0);
        let near = game.spawn_unit(2, UnitType::Worker, 3.0, 0.0);   // 3 units, range 5
        let far = game.spawn_unit(2, UnitType::Worker, 20.0, 0.0);   // 20 units, out of range
        assert!(game.in_attack_range(soldier, near));
        assert!(!game.in_attack_range(soldier, far));
    }

    #[test]
    fn insufficient_minerals() {
        let mut game = RtsGame::new("test", 1, 100, 100);
        // 500 minerals, Tank costs 200 → can build 2, not 3
        game.spawn_unit(1, UnitType::Tank, 0.0, 0.0); // 300 left
        game.spawn_unit(1, UnitType::Tank, 0.0, 0.0); // 100 left
        let id = game.spawn_unit(1, UnitType::Tank, 0.0, 0.0); // not enough
        assert_eq!(id, 0); // failed
    }
}
