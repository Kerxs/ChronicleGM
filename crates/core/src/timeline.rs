//! 剧情回溯与多时间线。
//!
//! - **检查点**：战役开始、每个完成的回合、手动剧情推进、战役结束时，保存完整战役状态
//!   （状态表全部行、回合数、骰子计数与种子、当前剧情节点、NPC 记忆与临时实体数量、会话摘要）。
//! - **回溯**：只能在战役结束后进行。选一个检查点，生成一条新时间线（新战役）：
//!   复制检查点状态、该点之前的全部事件、NPC 记忆、临时实体、会话摘要与之前的检查点，全部换新 ID。
//!   原时间线只读取，不修改；各时间线的数据按战役 ID 完全隔离。
//! - **校验**：检查点有校验值；从起点重放事件日志，与每个检查点比对（变量、任务、线索、物品、关系、
//!   实体状态、到访、解锁、走过的分支、当前节点、骰子计数）。
//! - **骰子**：默认恢复当时的种子与计数，新时间线之后的掷骰与原时间线当时的掷骰完全相同（可复现）；
//!   可以选择换新种子。

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::campaign::{self, Campaign};
use crate::error::{CoreError, Result};
use crate::util::{new_id, now_ms};
use crate::world::World;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 可比对的状态（重放事件得到的结果与检查点比较）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoreState {
    pub vars: BTreeMap<String, String>,
    pub quests: BTreeMap<String, String>,
    pub clues: BTreeSet<String>,
    /// (持有者, 物品) → 数量
    pub items: BTreeMap<String, i64>,
    /// "a|b|类型" → 是否成立
    pub relations: BTreeMap<String, bool>,
    /// "实体|字段" → 值（JSON 文本）
    pub entities: BTreeMap<String, String>,
    pub visits: BTreeMap<String, i64>,
    pub unlocks: BTreeSet<String>,
    pub edge_uses: BTreeMap<String, i64>,
    /// 人物关系 "from|to" → (类型, 强度, 信任, 好感, 敌意)。旧检查点没有这一项，按空处理。
    #[serde(default)]
    pub bonds: BTreeMap<String, (String, i64, i64, i64, i64)>,
    /// NPC 情绪：npc → (恐惧, 愤怒, 怀疑, 压力, 忠诚)。
    #[serde(default)]
    pub moods: BTreeMap<String, (i64, i64, i64, i64, i64)>,
    /// 战役记录："类型|ID" → JSON 文本。
    #[serde(default)]
    pub records: BTreeMap<String, String>,
    pub current_plot_node: Option<String>,
    pub roll_counter: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRow {
    pub session_no: i64,
    pub summary: String,
    pub status: String,
    pub upto_seq: i64,
    pub created_at: i64,
}

/// 检查点保存的完整状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointState {
    pub core: CoreState,
    pub turn: i64,
    pub rng_seed: i64,
    /// 线索揭示时的回合（显示用；不参与重放比对）。
    pub clue_turns: BTreeMap<String, i64>,
    /// 到访节点的首次回合（显示用；不参与重放比对）。
    pub visit_first_turn: BTreeMap<String, i64>,
    /// 当时已有的 NPC 记忆条数与临时实体条数（两者只追加，按写入顺序取前 N 条即可还原）。
    pub memories: i64,
    pub entities: i64,
    pub summaries: Vec<SummaryRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Checkpoint {
    pub id: i64,
    pub campaign_id: String,
    pub seq: i64,
    pub turn: i64,
    /// start / turn_end / transition / ending / fork / migrated
    pub label: String,
    pub checksum: String,
    pub app_version: String,
    pub created_at: i64,
}

/// FNV-1a 64 位校验值（用于发现意外修改，不是安全签名）。
pub fn checksum(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn rows2(conn: &Connection, sql: &str, id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 读取战役当前的完整状态。
pub fn capture(conn: &Connection, campaign_id: &str) -> Result<CheckpointState> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let mut core = CoreState { current_plot_node: c.current_plot_node.clone(), roll_counter: c.roll_counter, ..Default::default() };
    for (k, v) in rows2(conn, "SELECT key, value FROM state_vars WHERE campaign_id = ?1", campaign_id)? {
        core.vars.insert(k, v);
    }
    for (k, v) in rows2(conn, "SELECT quest_id, state FROM quest_states WHERE campaign_id = ?1", campaign_id)? {
        core.quests.insert(k, v);
    }
    let mut clue_turns = BTreeMap::new();
    for (k, v) in rows2(conn, "SELECT clue_id, CAST(turn AS TEXT) FROM clue_reveals WHERE campaign_id = ?1", campaign_id)? {
        core.clues.insert(k.clone());
        clue_turns.insert(k, v.parse().unwrap_or(0));
    }
    for (k, v) in rows2(conn, "SELECT holder || char(31) || item, CAST(qty AS TEXT) FROM inventory WHERE campaign_id = ?1", campaign_id)? {
        core.items.insert(k, v.parse().unwrap_or(0));
    }
    for (k, v) in rows2(
        conn,
        "SELECT a || char(31) || b || char(31) || rel_type, CAST(active AS TEXT) FROM relation_overrides WHERE campaign_id = ?1",
        campaign_id,
    )? {
        core.relations.insert(k, v == "1");
    }
    for (k, v) in rows2(conn, "SELECT entity_id || char(31) || field, value FROM entity_overrides WHERE campaign_id = ?1", campaign_id)? {
        core.entities.insert(k, v);
    }
    let mut visit_first_turn = BTreeMap::new();
    {
        let mut stmt = conn.prepare("SELECT plot_node_id, count, first_turn FROM plot_visits WHERE campaign_id = ?1")?;
        for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
            let (n, c, f) = r?;
            core.visits.insert(n.clone(), c);
            visit_first_turn.insert(n, f);
        }
    }
    for (k, _) in rows2(conn, "SELECT plot_node_id, '' FROM plot_unlocks WHERE campaign_id = ?1", campaign_id)? {
        core.unlocks.insert(k);
    }
    for (k, v) in rows2(conn, "SELECT edge_id, CAST(count AS TEXT) FROM edge_uses WHERE campaign_id = ?1", campaign_id)? {
        core.edge_uses.insert(k, v.parse().unwrap_or(0));
    }
    {
        let mut stmt = conn.prepare("SELECT from_id, to_id, kind, strength, trust, affection, hostility FROM campaign_bonds WHERE campaign_id = ?1")?;
        for r in stmt.query_map([campaign_id], |r| {
            Ok((format!("{}\u{1f}{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?), (r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        })? {
            let (k, v) = r?;
            core.bonds.insert(k, v);
        }
    }
    {
        let mut stmt = conn.prepare("SELECT npc_id, fear, anger, suspicion, stress, loyalty FROM campaign_moods WHERE campaign_id = ?1")?;
        for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))))? {
            let (k, v) = r?;
            core.moods.insert(k, v);
        }
    }
    for (k, v) in rows2(conn, "SELECT kind || char(31) || id, data FROM campaign_records WHERE campaign_id = ?1", campaign_id)? {
        core.records.insert(k, v);
    }
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [campaign_id], |r| r.get(0))?) };
    let summaries = {
        let mut stmt = conn.prepare(
            "SELECT session_no, summary, status, upto_seq, created_at FROM session_summaries WHERE campaign_id = ?1 ORDER BY session_no",
        )?;
        stmt.query_map([campaign_id], |r| {
            Ok(SummaryRow { session_no: r.get(0)?, summary: r.get(1)?, status: r.get(2)?, upto_seq: r.get(3)?, created_at: r.get(4)? })
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    Ok(CheckpointState {
        core,
        turn: c.turn,
        rng_seed: c.rng_seed,
        clue_turns,
        visit_first_turn,
        memories: count("SELECT COUNT(*) FROM npc_memories WHERE campaign_id = ?1")?,
        entities: count("SELECT COUNT(*) FROM campaign_entities WHERE campaign_id = ?1")?,
        summaries,
    })
}

fn last_seq(conn: &Connection, campaign_id: &str) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM event_log WHERE campaign_id = ?1", [campaign_id], |r| r.get(0))?)
}

fn insert_checkpoint(conn: &Connection, campaign_id: &str, seq: i64, label: &str, st: &CheckpointState) -> Result<i64> {
    let json = serde_json::to_string(st)?;
    conn.execute(
        "INSERT INTO campaign_checkpoints (campaign_id, seq, turn, label, state, checksum, app_version, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![campaign_id, seq, st.turn, label, json, checksum(&json), APP_VERSION, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 在当前位置保存检查点（调用方负责事务）。
pub fn checkpoint(conn: &Connection, campaign_id: &str, label: &str) -> Result<i64> {
    let st = capture(conn, campaign_id)?;
    insert_checkpoint(conn, campaign_id, last_seq(conn, campaign_id)?, label, &st)
}

fn cp_row(r: &rusqlite::Row) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        id: r.get("id")?,
        campaign_id: r.get("campaign_id")?,
        seq: r.get("seq")?,
        turn: r.get("turn")?,
        label: r.get("label")?,
        checksum: r.get("checksum")?,
        app_version: r.get("app_version")?,
        created_at: r.get("created_at")?,
    })
}

pub fn checkpoints(conn: &Connection, campaign_id: &str) -> Result<Vec<Checkpoint>> {
    let mut stmt = conn.prepare("SELECT * FROM campaign_checkpoints WHERE campaign_id = ?1 ORDER BY seq, id")?;
    let rows = stmt.query_map([campaign_id], cp_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 读取检查点状态并核对校验值。
pub fn load_checkpoint(conn: &Connection, id: i64) -> Result<(Checkpoint, CheckpointState)> {
    let (cp, json): (Checkpoint, String) = conn
        .query_row("SELECT * FROM campaign_checkpoints WHERE id = ?1", [id], |r| Ok((cp_row(r)?, r.get("state")?)))
        .optional()?
        .ok_or_else(|| CoreError::NotFound("检查点", id.to_string()))?;
    if checksum(&json) != cp.checksum {
        return Err(CoreError::Invalid(format!("检查点 {id} 的校验值不符，数据可能被修改过，不能用于回溯")));
    }
    Ok((cp, serde_json::from_str(&json)?))
}

/// 升级前就存在的战役没有历史检查点：补一个"当前状态"检查点（之前的位置无法回溯，如实标注）。
pub fn ensure_baseline(conn: &Connection, campaign_id: &str) -> Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM campaign_checkpoints WHERE campaign_id = ?1", [campaign_id], |r| r.get(0))?;
    if n == 0 {
        checkpoint(conn, campaign_id, "migrated")?;
    }
    Ok(())
}

/// 结束战役：记录结局事件，保存结束时的完整快照。之后只能查看或回溯。
pub fn end_campaign(world: &mut World, campaign_id: &str, ending: &str) -> Result<Checkpoint> {
    let tx = world.conn.transaction()?;
    campaign::ensure_active(&tx, campaign_id)?;
    ensure_baseline(&tx, campaign_id)?;
    let c = campaign::get_campaign(&tx, campaign_id)?;
    let ending = if ending.trim().is_empty() {
        c.current_plot_node
            .as_ref()
            .and_then(|n| crate::plot::get_plot_node(&tx, n).ok().flatten())
            .map(|n| n.title)
            .unwrap_or_else(|| "（未注明结局）".into())
    } else {
        ending.trim().chars().take(200).collect()
    };
    campaign::record(&tx, campaign_id, "campaign_end", &serde_json::json!({ "ending": ending }))?;
    tx.execute("UPDATE campaigns SET status = 'ended', ended_at = ?2, ending = ?3, updated_at = ?2 WHERE id = ?1", params![campaign_id, now_ms(), ending])?;
    let id = checkpoint(&tx, campaign_id, "ending")?;
    let cp = tx.query_row("SELECT * FROM campaign_checkpoints WHERE id = ?1", [id], cp_row)?;
    tx.commit()?;
    Ok(cp)
}

/// 可以回溯到的历史节点（带简短预览，便于选择）。
/// 手动存档：在当前位置保存一个带名字的检查点（label = "save:<名字>"；快速存档的名字是"快速存档"）。
pub fn save_point(world: &mut World, campaign_id: &str, name: &str) -> Result<Checkpoint> {
    let name: String = name.trim().chars().take(40).collect();
    let name = if name.is_empty() { "快速存档".to_string() } else { name };
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    if c.status != "active" {
        return Err(CoreError::Invalid("这条时间线已经结束，不能再存档（可以从它的存档点读档开新时间线）".into()));
    }
    if campaign::incomplete_turn(world.conn(), campaign_id)?.is_some() {
        return Err(CoreError::Invalid("有一个没完成的回合，先处理它再存档".into()));
    }
    let tx = world.conn.transaction()?;
    let id = checkpoint(&tx, campaign_id, &format!("save:{name}"))?;
    tx.commit()?;
    let (cp, _) = load_checkpoint(world.conn(), id)?;
    Ok(cp)
}

/// 最近一次快速存档。
pub fn last_quicksave(conn: &Connection, campaign_id: &str) -> Result<Option<Checkpoint>> {
    Ok(conn.query_row("SELECT * FROM campaign_checkpoints WHERE campaign_id = ?1 AND label = 'save:快速存档' ORDER BY id DESC LIMIT 1", [campaign_id], cp_row).optional()?)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ForkPoint {
    pub checkpoint: Checkpoint,
    /// 这一点之前最后一次玩家行动。
    pub last_action: String,
    /// 这一点之前最后一段叙述（玩家可见）。
    pub last_narration: String,
}

pub fn fork_points(conn: &Connection, campaign_id: &str) -> Result<Vec<ForkPoint>> {
    let cps = checkpoints(conn, campaign_id)?;
    let mut out = vec![];
    let last = |kind: &str, seq: i64| -> Result<String> {
        Ok(conn
            .query_row(
                "SELECT payload FROM event_log WHERE campaign_id = ?1 AND kind = ?2 AND seq <= ?3 ORDER BY seq DESC LIMIT 1",
                params![campaign_id, kind, seq],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|p| serde_json::from_str::<Value>(&p).ok())
            .and_then(|v| v.get("text").and_then(Value::as_str).map(|s| s.chars().take(80).collect()))
            .unwrap_or_default())
    };
    for cp in cps {
        out.push(ForkPoint { last_action: last("player", cp.seq)?, last_narration: last("narration", cp.seq)?, checkpoint: cp });
    }
    Ok(out)
}

/// 从某个检查点开出新时间线（进行中的战役也可以：读档 = 从存档点开出新时间线）。原时间线不被修改。
pub fn fork(world: &mut World, source_id: &str, checkpoint_id: i64, name: &str, reseed: bool) -> Result<Campaign> {
    if campaign::get_campaign(world.conn(), source_id)?.status == "active" && campaign::incomplete_turn(world.conn(), source_id)?.is_some() {
        return Err(CoreError::Invalid("这条时间线有一个没完成的回合，先处理它（重新输入或点「知道了」）再读档".into()));
    }
    let tx = world.conn.transaction()?;
    let src = campaign::get_campaign(&tx, source_id)?;
    let (cp, st) = load_checkpoint(&tx, checkpoint_id)?;
    if cp.campaign_id != source_id {
        return Err(CoreError::Invalid("这个检查点不属于该时间线".into()));
    }
    let name = if name.trim().is_empty() { format!("{}（回到第 {} 回合）", src.name, cp.turn) } else { name.trim().to_string() };
    let new = new_id();
    let now = now_ms();
    let seed = if reseed { (ulid::Ulid::generate().random() as i64) ^ now } else { st.rng_seed };
    tx.execute(
        "INSERT INTO campaigns (id, script_id, name, base_commit, rng_seed, roll_counter, turn, current_plot_node, created_at, updated_at,
                                parent_id, fork_seq, fork_turn, status)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,?10,?11,?12,'active')",
        params![new, src.script_id, name, src.base_commit, seed, st.core.roll_counter, st.turn, st.core.current_plot_node, now, source_id, cp.seq, cp.turn],
    )?;
    // 1. 状态表。
    restore_state(&tx, &new, &st)?;
    // 2. 分叉点之前的事件（保持顺序，记录新旧序号对应）。
    let mut seq_map: BTreeMap<i64, i64> = BTreeMap::new();
    {
        let mut stmt = tx.prepare("SELECT seq, turn, kind, payload, created_at FROM event_log WHERE campaign_id = ?1 AND seq <= ?2 ORDER BY seq")?;
        let rows: Vec<(i64, i64, String, String, i64)> =
            stmt.query_map(params![source_id, cp.seq], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
        for (seq, turn, kind, payload, at) in rows {
            tx.execute(
                "INSERT INTO event_log (campaign_id, turn, kind, payload, created_at) VALUES (?1,?2,?3,?4,?5)",
                params![new, turn, kind, payload, at],
            )?;
            seq_map.insert(seq, tx.last_insert_rowid());
        }
    }
    let map_seq = |old: i64| -> i64 { seq_map.range(..=old).next_back().map(|(_, v)| *v).unwrap_or(0) };
    // 3. NPC 记忆与临时实体（只追加，按写入顺序取前 N 条；逐条插入以保持顺序，之后再回溯时同样适用）。
    {
        let mut stmt = tx.prepare("SELECT npc_id, content, importance, turn, created_at FROM npc_memories WHERE campaign_id = ?1 ORDER BY rowid LIMIT ?2")?;
        let rows: Vec<(String, String, i64, i64, i64)> =
            stmt.query_map(params![source_id, st.memories], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
        for (npc, content, imp, turn, at) in rows {
            tx.execute(
                "INSERT INTO npc_memories (id, campaign_id, npc_id, content, importance, turn, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![new_id(), new, npc, content, imp, turn, at],
            )?;
        }
        let mut stmt = tx.prepare(
            "SELECT kind, name, summary, gm_text, changeset_id, created_turn, created_at FROM campaign_entities WHERE campaign_id = ?1 ORDER BY rowid LIMIT ?2",
        )?;
        // (类型, 名称, 简介, 主持人信息, 变更集, 创建回合, 创建时间)
        type EntityRow = (String, String, String, String, Option<String>, i64, i64);
        let rows: Vec<EntityRow> = stmt
            .query_map(params![source_id, st.entities], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (kind, name, summary, gm, cs, turn, at) in rows {
            tx.execute(
                "INSERT INTO campaign_entities (id, campaign_id, kind, name, summary, gm_text, changeset_id, created_turn, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![new_id(), new, kind, name, summary, gm, cs, turn, at],
            )?;
        }
    }
    // 4. 会话摘要（覆盖范围换成新时间线的事件序号）。
    for s in &st.summaries {
        tx.execute(
            "INSERT INTO session_summaries (id, campaign_id, session_no, summary, status, created_at, upto_seq) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![new_id(), new, s.session_no, s.summary, s.status, s.created_at, map_seq(s.upto_seq)],
        )?;
    }
    // 5. 之前的检查点（新时间线也能再回溯到更早的位置）。
    for old in checkpoints(&tx, source_id)?.into_iter().filter(|c| c.seq <= cp.seq && c.id != cp.id) {
        let (_, ost) = load_checkpoint(&tx, old.id)?;
        let mut ost = ost;
        for s in &mut ost.summaries {
            s.upto_seq = map_seq(s.upto_seq);
        }
        insert_checkpoint(&tx, &new, map_seq(old.seq), &old.label, &ost)?;
    }
    // 6. 分叉事件与新时间线的起点检查点。
    campaign::record(
        &tx,
        &new,
        "timeline_fork",
        &serde_json::json!({ "from_campaign": source_id, "from_name": src.name, "from_seq": cp.seq, "from_turn": cp.turn, "checkpoint": cp.id, "reseed": reseed, "seed": seed }),
    )?;
    checkpoint(&tx, &new, "fork")?;
    // 7. 自检：新时间线的状态必须与检查点完全一致，否则整个回溯撤销。
    let got = capture(&tx, &new)?;
    let mut want = st.clone();
    want.rng_seed = seed;
    for s in &mut want.summaries {
        s.upto_seq = map_seq(s.upto_seq);
    }
    if got != want {
        return Err(CoreError::Invalid("回溯后的状态与检查点不一致，已撤销（没有写入任何数据）".into()));
    }
    let c = campaign::get_campaign(&tx, &new)?;
    tx.commit()?;
    Ok(c)
}

fn restore_state(conn: &Connection, id: &str, st: &CheckpointState) -> Result<()> {
    let split = |k: &str| -> Vec<String> { k.split('\u{1f}').map(str::to_string).collect() };
    for (k, v) in &st.core.vars {
        conn.execute("INSERT INTO state_vars (campaign_id, key, value) VALUES (?1,?2,?3)", params![id, k, v])?;
    }
    for (k, v) in &st.core.quests {
        conn.execute("INSERT INTO quest_states (campaign_id, quest_id, state) VALUES (?1,?2,?3)", params![id, k, v])?;
    }
    for k in &st.core.clues {
        conn.execute("INSERT INTO clue_reveals (campaign_id, clue_id, turn) VALUES (?1,?2,?3)", params![id, k, st.clue_turns.get(k).copied().unwrap_or(0)])?;
    }
    for (k, q) in &st.core.items {
        let p = split(k);
        conn.execute("INSERT INTO inventory (campaign_id, holder, item, qty) VALUES (?1,?2,?3,?4)", params![id, p[0], p.get(1), q])?;
    }
    for (k, a) in &st.core.relations {
        let p = split(k);
        conn.execute("INSERT INTO relation_overrides (campaign_id, a, b, rel_type, active) VALUES (?1,?2,?3,?4,?5)", params![id, p[0], p.get(1), p.get(2), a])?;
    }
    for (k, v) in &st.core.entities {
        let p = split(k);
        conn.execute("INSERT INTO entity_overrides (campaign_id, entity_id, field, value) VALUES (?1,?2,?3,?4)", params![id, p[0], p.get(1), v])?;
    }
    for (k, n) in &st.core.visits {
        conn.execute(
            "INSERT INTO plot_visits (campaign_id, plot_node_id, count, first_turn) VALUES (?1,?2,?3,?4)",
            params![id, k, n, st.visit_first_turn.get(k).copied().unwrap_or(0)],
        )?;
    }
    for k in &st.core.unlocks {
        conn.execute("INSERT INTO plot_unlocks (campaign_id, plot_node_id) VALUES (?1,?2)", params![id, k])?;
    }
    for (k, n) in &st.core.edge_uses {
        conn.execute("INSERT INTO edge_uses (campaign_id, edge_id, count) VALUES (?1,?2,?3)", params![id, k, n])?;
    }
    for (k, (f, a, s, t, l)) in &st.core.moods {
        conn.execute(
            "INSERT INTO campaign_moods (campaign_id, npc_id, fear, anger, suspicion, stress, loyalty) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id, k, f, a, s, t, l],
        )?;
    }
    for (k, v) in &st.core.records {
        let p = split(k);
        conn.execute("INSERT INTO campaign_records (campaign_id, kind, id, data) VALUES (?1,?2,?3,?4)", params![id, p[0], p.get(1), v])?;
    }
    for (k, (kind, s, t, a, h)) in &st.core.bonds {
        let p = split(k);
        conn.execute(
            "INSERT INTO campaign_bonds (campaign_id, from_id, to_id, kind, strength, trust, affection, hostility) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, p[0], p.get(1), kind, s, t, a, h],
        )?;
    }
    Ok(())
}

// ===================== 重放与校验 =====================

/// 把一条事件应用到内存中的状态（与 campaign::apply_changes / take_edge 的写入规则一致）。
fn replay_event(s: &mut CoreState, kind: &str, payload: &Value) {
    let apply = |s: &mut CoreState, changes: &Value| {
        let Ok(list) = serde_json::from_value::<Vec<chronicle_rules::StateChange>>(changes.clone()) else { return };
        for ch in list {
            use chronicle_rules::StateChange::*;
            match ch {
                SetVar { key, new, .. } => {
                    s.vars.insert(key, crate::entity::js(&new));
                }
                SetQuestState { quest, new, .. } => {
                    s.quests.insert(quest, new);
                }
                SetItemQty { holder, item, new, .. } => {
                    s.items.insert(format!("{holder}\u{1f}{item}"), new);
                }
                RevealClue { clue } => {
                    s.clues.insert(clue);
                }
                SetRelation { a, b, rel_type, active } => {
                    s.relations.insert(format!("{a}\u{1f}{b}\u{1f}{rel_type}"), active);
                }
                SetEntityField { entity, field, value } => {
                    s.entities.insert(format!("{entity}\u{1f}{field}"), crate::entity::js(&value));
                }
                UnlockNode { node } => {
                    s.unlocks.insert(node);
                }
                AdjustMood { npc, new, .. } => {
                    s.moods.insert(npc, (new.fear, new.anger, new.suspicion, new.stress, new.loyalty));
                }
                UpsertRecord { kind, id, new, .. } => {
                    s.records.insert(format!("{kind}\u{1f}{id}"), crate::entity::js(&new));
                }
                DeleteRecord { kind, id, .. } => {
                    s.records.remove(&format!("{kind}\u{1f}{id}"));
                }
                AdjustBond { from, to, kind, new, .. } => {
                    s.bonds.insert(format!("{from}\u{1f}{to}"), (kind, new.strength, new.trust, new.affection, new.hostility));
                }
                Spawn { .. } => {}
            }
        }
    };
    match kind {
        "effects" => apply(s, &payload["changes"]),
        "roll" => s.roll_counter += 1,
        "transition" | "transition_failed" => {
            let used = match payload.get("stream") {
                Some(Value::Null) => false,
                Some(_) => true,
                // 旧版本事件没有记录流号：有检定记录就说明用过一次骰子。
                None => payload["outcome"]["checks"].as_array().is_some_and(|a| !a.is_empty()),
            };
            if used {
                s.roll_counter += 1;
            }
            if kind == "transition" {
                apply(s, &payload["outcome"]["changes"]);
                if let Some(to) = payload["outcome"]["to"].as_str() {
                    *s.visits.entry(to.to_string()).or_insert(0) += 1;
                    s.current_plot_node = Some(to.to_string());
                }
                if let Some(e) = payload["edge"].as_str() {
                    *s.edge_uses.entry(e.to_string()).or_insert(0) += 1;
                }
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct VerifyReport {
    pub ok: bool,
    pub checkpoints: usize,
    pub events: usize,
    /// 校验值不符的检查点。
    pub checksum_failures: Vec<i64>,
    /// 重放结果与检查点不一致之处。
    pub mismatches: Vec<String>,
    /// 说明（例如升级前的战役只能从补建的检查点开始校验）。
    pub notes: Vec<String>,
}

fn diff_core(a: &CoreState, b: &CoreState) -> Vec<String> {
    let mut out = vec![];
    macro_rules! cmp {
        ($f:ident, $label:expr) => {
            if a.$f != b.$f {
                out.push($label.to_string());
            }
        };
    }
    cmp!(vars, "变量");
    cmp!(quests, "任务状态");
    cmp!(clues, "已知线索");
    cmp!(items, "物品");
    cmp!(relations, "关系");
    cmp!(entities, "实体状态");
    cmp!(visits, "到访记录");
    cmp!(unlocks, "解锁的节点");
    cmp!(edge_uses, "走过的分支");
    cmp!(bonds, "人物关系");
    cmp!(moods, "NPC 情绪");
    cmp!(records, "调查与任务记录");
    cmp!(current_plot_node, "当前剧情节点");
    cmp!(roll_counter, "骰子计数");
    out
}

/// 从第一个检查点开始重放事件日志，与之后每个检查点比对；同时核对校验值。
pub fn verify(conn: &Connection, campaign_id: &str) -> Result<VerifyReport> {
    let cps = checkpoints(conn, campaign_id)?;
    let mut rep = VerifyReport { ok: true, checkpoints: cps.len(), events: 0, checksum_failures: vec![], mismatches: vec![], notes: vec![] };
    let mut states = vec![];
    for cp in &cps {
        match load_checkpoint(conn, cp.id) {
            Ok((_, st)) => states.push((cp.clone(), st)),
            Err(_) => rep.checksum_failures.push(cp.id),
        }
    }
    let Some((first, first_state)) = states.first().cloned() else {
        rep.notes.push("这条时间线还没有检查点".into());
        return Ok(rep);
    };
    if first.label == "migrated" {
        rep.notes.push(format!("这是升级前创建的战役，只能从第 {} 回合（补建的检查点）开始校验与回溯", first.turn));
    }
    let mut cur = first_state.core.clone();
    let mut stmt = conn.prepare("SELECT seq, kind, payload FROM event_log WHERE campaign_id = ?1 AND seq > ?2 ORDER BY seq")?;
    let events: Vec<(i64, String, String)> =
        stmt.query_map(params![campaign_id, first.seq], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    rep.events = events.len();
    let mut idx = 1;
    let check = |cur: &CoreState, cp: &Checkpoint, st: &CheckpointState, rep: &mut VerifyReport| {
        let d = diff_core(cur, &st.core);
        if !d.is_empty() {
            rep.mismatches.push(format!("第 {} 回合的检查点（{}）：{}", cp.turn, cp.label, d.join("、")));
        }
    };
    for (seq, kind, payload) in &events {
        while idx < states.len() && states[idx].0.seq < *seq {
            check(&cur, &states[idx].0, &states[idx].1, &mut rep);
            idx += 1;
        }
        replay_event(&mut cur, kind, &serde_json::from_str(payload).unwrap_or(Value::Null));
    }
    while idx < states.len() {
        check(&cur, &states[idx].0, &states[idx].1, &mut rep);
        idx += 1;
    }
    rep.ok = rep.checksum_failures.is_empty() && rep.mismatches.is_empty();
    Ok(rep)
}

// ===================== 管理与比较 =====================

pub fn rename(conn: &Connection, campaign_id: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(CoreError::Invalid("名称不能为空".into()));
    }
    campaign::get_campaign(conn, campaign_id)?;
    conn.execute("UPDATE campaigns SET name = ?2, updated_at = ?3 WHERE id = ?1", params![campaign_id, name.trim(), now_ms()])?;
    Ok(())
}

pub fn set_archived(conn: &Connection, campaign_id: &str, archived: bool) -> Result<()> {
    campaign::get_campaign(conn, campaign_id)?;
    conn.execute("UPDATE campaigns SET archived = ?2, updated_at = ?3 WHERE id = ?1", params![campaign_id, archived, now_ms()])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CompareRow {
    pub category: String,
    pub key: String,
    /// 每条时间线的值（None 表示没有）。
    pub values: Vec<Option<String>>,
    pub differs: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TimelineComparison {
    pub campaigns: Vec<Campaign>,
    pub rows: Vec<CompareRow>,
}

/// 比较多条时间线的最终状态（结局、剧情位置、任务、线索、物品、变量、关系、实体状态）。名称会换成可读的标题。
pub fn compare(conn: &Connection, ids: &[String]) -> Result<TimelineComparison> {
    let mut campaigns = vec![];
    let mut states = vec![];
    for id in ids {
        campaigns.push(campaign::get_campaign(conn, id)?);
        states.push(capture(conn, id)?);
    }
    let title = |id: &str| -> String {
        conn.query_row(
            "SELECT title FROM script_items WHERE id = ?1 UNION ALL SELECT title FROM plot_nodes WHERE id = ?1 UNION ALL SELECT title FROM nodes WHERE id = ?1 LIMIT 1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| id.to_string())
    };
    let mut rows = vec![];
    let mut push = |category: &str, key: String, values: Vec<Option<String>>| {
        let differs = values.windows(2).any(|w| w[0] != w[1]);
        rows.push(CompareRow { category: category.into(), key, values, differs });
    };
    push("结局", "结局".into(), campaigns.iter().map(|c| (c.status == "ended").then(|| c.ending.clone())).collect());
    push("进度", "回合".into(), campaigns.iter().map(|c| Some(c.turn.to_string())).collect());
    push("进度", "当前剧情节点".into(), states.iter().map(|s| s.core.current_plot_node.as_deref().map(title)).collect());
    let keys = |f: &dyn Fn(&CheckpointState) -> Vec<String>| -> BTreeSet<String> { states.iter().flat_map(f).collect() };
    for k in keys(&|s| s.core.quests.keys().cloned().collect()) {
        push("任务", title(&k), states.iter().map(|s| s.core.quests.get(&k).cloned()).collect());
    }
    for k in keys(&|s| s.core.clues.iter().cloned().collect()) {
        push("线索", title(&k), states.iter().map(|s| s.core.clues.contains(&k).then(|| "已知".to_string())).collect());
    }
    for k in keys(&|s| s.core.items.keys().cloned().collect()) {
        let p: Vec<&str> = k.split('\u{1f}').collect();
        push("物品", format!("{} 的 {}", if p[0] == "party" { "队伍".into() } else { title(p[0]) }, title(p.get(1).unwrap_or(&""))), states.iter().map(|s| s.core.items.get(&k).map(|q| q.to_string())).collect());
    }
    for k in keys(&|s| s.core.vars.keys().cloned().collect()) {
        let label = match k.strip_prefix("attitude.") {
            Some(npc) => format!("{} 的态度", title(npc)),
            None => k.clone(),
        };
        push("变量", label, states.iter().map(|s| s.core.vars.get(&k).cloned()).collect());
    }
    for k in keys(&|s| s.core.relations.keys().cloned().collect()) {
        let p: Vec<&str> = k.split('\u{1f}').collect();
        push("关系", format!("{} — {}（{}）", title(p[0]), title(p.get(1).unwrap_or(&"")), p.get(2).unwrap_or(&"")), states.iter().map(|s| s.core.relations.get(&k).map(|a| if *a { "成立" } else { "不成立" }.to_string())).collect());
    }
    for k in keys(&|s| s.core.bonds.keys().cloned().collect()) {
        let p: Vec<&str> = k.split('\u{1f}').collect();
        let who = |x: &str| if x == "party" { "玩家".to_string() } else { title(x) };
        push(
            "人物关系",
            format!("{} 对 {}", who(p[0]), who(p.get(1).unwrap_or(&""))),
            states.iter().map(|s| s.core.bonds.get(&k).map(|(kind, st, t, a, h)| format!("{kind} 强度{st} 信任{t} 好感{a} 敌意{h}"))).collect(),
        );
    }
    for k in keys(&|s| s.core.moods.keys().cloned().collect()) {
        push(
            "NPC 情绪",
            title(&k),
            states.iter().map(|s| s.core.moods.get(&k).map(|(f, a, su, st, l)| format!("恐惧{f} 愤怒{a} 怀疑{su} 压力{st} 忠诚{l}"))).collect(),
        );
    }
    for k in keys(&|s| s.core.records.keys().cloned().collect()) {
        let p: Vec<&str> = k.split('\u{1f}').collect();
        let (kind, id) = (p[0], p.get(1).copied().unwrap_or(""));
        // 世界事件、成长记录可能有上百条：合并成计数行（见下）。
        if matches!(kind, "world_event" | "growth" | "shop_init" | "item_source" | "director_note" | "director_beat") {
            continue;
        }
        let label = |v: &String| -> String {
            let j: Value = serde_json::from_str(v).unwrap_or(Value::Null);
            match kind {
                "place" => match j["state"].as_str() {
                    Some("explored") => "已探索".into(),
                    Some("heard") => "未探索".into(),
                    other => other.unwrap_or("（有）").into(),
                },
                "faction" => {
                    let pw = &j["power"];
                    format!("财富{} 军力{} 影响{} 稳定{}；{}", pw["wealth"], pw["military"], pw["influence"], pw["stability"], j["last_action"].as_str().unwrap_or(""))
                }
                "marker" => j["label"].as_str().unwrap_or("（有）").into(),
                "trigger_fired" => format!("第 {} 天触发", j["day"]),
                "sheet" => format!("{} 级，经验 {}，生命 {}/{}，伤势 {}", j["level"], j["xp"], j["hp"], j["max_hp"], j["wounds"].as_array().map(|a| a.len()).unwrap_or(0)),
                "equip" => j.as_object().map(|o| o.values().filter_map(|v| v.as_str()).map(title).collect::<Vec<_>>().join("、")).unwrap_or_default(),
                "combat" => format!("{}，{} 轮", j["status"].as_str().unwrap_or(""), j["round"]),
                _ => j.get("title").or_else(|| j.get("text")).and_then(Value::as_str).map(|t| t.chars().take(40).collect()).unwrap_or_else(|| "（有）".into()),
            }
        };
        let (category, name) = match kind {
            "place" => ("世界状态", format!("地点 · {}", title(id))),
            "faction" => ("世界状态", format!("势力 · {}", title(id))),
            "marker" => ("世界状态", format!("地图标记 · {id}")),
            "trigger_fired" => ("世界状态", format!("触发器 · {}", title(id))),
            "sheet" => ("人物与战斗", format!("角色卡 · {}", if id == "party" { "玩家".to_string() } else { title(id) })),
            "equip" => ("人物与战斗", format!("装备 · {}", if id == "party" { "玩家".to_string() } else { title(id) })),
            "combat" => ("人物与战斗", format!("战斗 · {id}")),
            _ => ("调查与任务记录", format!("{kind} · {id}")),
        };
        push(category, name, states.iter().map(|s| s.core.records.get(&k).map(label)).collect());
    }
    {
        let counts: Vec<Option<String>> = states.iter().map(|s| Some(s.core.records.keys().filter(|k| k.starts_with("world_event\u{1f}")).count().to_string())).collect();
        if counts.iter().any(|c| c.as_deref() != Some("0")) {
            push("世界状态", "世界事件数".into(), counts);
        }
        let growth: Vec<Option<String>> = states.iter().map(|s| Some(s.core.records.keys().filter(|k| k.starts_with("growth\u{1f}")).count().to_string())).collect();
        if growth.iter().any(|c| c.as_deref() != Some("0")) {
            push("人物与战斗", "成长记录数".into(), growth);
        }
    }
    for k in keys(&|s| s.core.entities.keys().cloned().collect()) {
        let p: Vec<&str> = k.split('\u{1f}').collect();
        push("实体状态", format!("{} · {}", title(p[0]), p.get(1).unwrap_or(&"")), states.iter().map(|s| s.core.entities.get(&k).cloned()).collect());
    }
    Ok(TimelineComparison { campaigns, rows })
}

#[cfg(test)]
mod tests {
    #[test]
    fn checksum_is_stable_and_sensitive() {
        assert_eq!(super::checksum("abc"), super::checksum("abc"));
        assert_ne!(super::checksum("abc"), super::checksum("abd"));
        assert_eq!(super::checksum("").len(), 16);
    }
}
