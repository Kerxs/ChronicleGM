//! 回合制战斗（全部由程序结算，AI 只负责描述）：
//! - 先攻：每个参战者掷 1d20 + 敏捷修正，从高到低行动。
//! - 行动点：每回合按规则设置恢复；攻击、移动、防御、使用物品、自定义行动各有消耗。
//! - 战场位置：网格坐标，距离按切比雪夫距离（斜走也算一格）；近战射程 1，远程武器看射程。
//! - 伤害与状态：命中 = 1d20 + 攻击加值 ≥ 防御；自然骰达到暴击点数时伤害骰掷两次，自然 1 必定失手。
//!   状态可以每轮造成伤害、修正攻击与防御、跳过回合；按轮数到期。
//! - 敌人和同伴由程序控制（靠近最近的对手并攻击），不调用模型。
//! - 结束时：生命值、伤势写回角色卡；胜利由程序计算经验与战利品。
//!
//! 战斗状态存为战役记录 `combat`，进行中的战斗记在受保护变量 `combat.active`，每一步都写入事件日志，
//! 骰子用战役种子，随时间线回溯恢复。

use chronicle_rules::{Effect, StateView};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::rpg::{self, PARTY, RecipeInput, RuleSet, Sheet, Status, Wound};
use crate::world::World;

pub const ACTIVE_VAR: &str = "combat.active";
const MAX_LOG: usize = 80;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Combatant {
    pub id: String,
    /// 对应的角色卡（玩家 party 或人物节点 ID）；临时敌人没有。
    pub actor: Option<String>,
    pub name: String,
    /// party（玩家）/ ally（同伴）/ enemy（敌人）
    pub side: String,
    pub hp: i64,
    pub max_hp: i64,
    pub ac: i64,
    pub attack: i64,
    pub weapon: String,
    pub damage: String,
    pub range: i64,
    pub on_hit: Option<Status>,
    pub ap: i64,
    pub init: i64,
    pub x: i64,
    pub y: i64,
    pub statuses: Vec<Status>,
    /// 空：还在战斗；down（倒下）/ fled（逃走）/ dead（死亡）
    pub out: String,
    pub level: i64,
    pub xp: i64,
    pub loot: Vec<RecipeInput>,
    /// 这场战斗中新增的伤势（结束时写回角色卡）。
    pub new_wounds: Vec<Wound>,
    /// 生命降到四分之一时检定过士气（只检定一次）。
    #[serde(default)]
    pub morale_checked: bool,
    /// 首领：首领还在战斗时无法逃跑。
    #[serde(default)]
    pub boss: bool,
    /// 武器用的弹药名（空 = 不用弹药）；每次攻击从背包扣 1 发，没有弹药就打不响。
    #[serde(default)]
    pub ammo: String,
    /// 速度：决定行动顺序（先攻 = 1d20 + 速度）与闪避检定。= 敏捷修正（敌人按等级）。
    #[serde(default)]
    pub speed: i64,
    /// 正在闪避（到自己下一次行动前）：被攻击时掷闪避检定（1d20 + 速度）对抗攻击总值。
    #[serde(default)]
    pub dodging: bool,
}

impl Combatant {
    fn active(&self) -> bool {
        self.out.is_empty()
    }
    fn mods(&self) -> i64 {
        self.statuses.iter().map(|s| s.modifier).sum()
    }
    fn ac_now(&self) -> i64 {
        self.ac + self.statuses.iter().map(|s| s.ac).sum::<i64>()
    }
    fn friendly(&self) -> bool {
        self.side != "enemy"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Combat {
    pub id: String,
    pub round: i64,
    /// 当前行动者在 order 中的位置。
    pub turn: usize,
    pub order: Vec<String>,
    pub units: Vec<Combatant>,
    pub log: Vec<String>,
    /// active / victory / defeat / fled / ended
    pub status: String,
    pub width: i64,
    pub height: i64,
    pub reason: String,
    /// 结束时的结算（经验、战利品）。
    pub rewards: Vec<String>,
    /// 障碍物（阻挡移动，挡在远程攻击路线上时提供掩体 +2 防御）。
    #[serde(default)]
    pub obstacles: Vec<(i64, i64)>,
    /// 场地封锁等原因导致无法逃跑（空 = 可以尝试逃跑）。
    #[serde(default)]
    pub no_escape: String,
    /// 战场上可以互动的物件（场景交互）。
    #[serde(default)]
    pub props: Vec<Prop>,
}

/// 战场物件：走到旁边（距离 ≤ 1）就能互动，效果由程序结算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Prop {
    pub id: String,
    pub name: String,
    /// cover（躲到后面：防御 +2，两轮；也可以砸毁 destroy）/ fire（点燃：周围 2 格内的一个敌人受 1d6 伤害并灼烧）/ drop（推倒重物：旁边的敌人敏捷检定，失败倒地一轮）
    /// / mechanism（战场上的机关：拉动后周围 2 格的敌人检定，失败受 1d8 伤害）
    pub kind: String,
    pub x: i64,
    pub y: i64,
    /// 一次性的物件用掉了。
    #[serde(default)]
    pub used: bool,
    pub desc: String,
}

/// 物件模板（按战役种子挑 1–3 个放到战场上）。
const PROP_KINDS: [(&str, &str, &str); 4] = [
    ("cover", "货箱", "躲到后面：防御 +2，持续两轮；也可以砸毁它"),
    ("mechanism", "生锈的机关杆", "拉动机关：周围 2 格内的敌人要过敏捷检定（难度 13），失败受到 1d8 伤害"),
    ("fire", "油灯", "打翻点燃：周围 2 格内的一个敌人受到 1d6 伤害并灼烧"),
    ("drop", "悬挂的重物", "割断绳子：旁边的敌人要过敏捷检定（难度 12），失败就倒地一轮"),
];

/// 所有人都会的技能（剧本规则里的自定义行动之外）。
pub fn builtin_skills(r: &RuleSet) -> Vec<rpg::CustomAction> {
    vec![
        rpg::CustomAction { name: "瞄准".into(), ap: 1, kind: "status".into(), dice: String::new(), attr: String::new(), range: 0, status: Some(Status { name: "瞄准".into(), rounds: Some(2), modifier: 3, ..Default::default() }) },
        rpg::CustomAction { name: "包扎".into(), ap: 2, kind: "heal".into(), dice: "1d4".into(), attr: rpg::role_attr(r, "insight"), range: 1, status: None },
    ]
}

/// 战斗中可以用的技能：剧本规则里的自定义行动 + 通用技能（同名时以剧本为准）。
pub fn skills(r: &RuleSet) -> Vec<rpg::CustomAction> {
    let mut out = r.custom_actions.clone();
    for s in builtin_skills(r) {
        if !out.iter().any(|x| x.name == s.name) {
            out.push(s);
        }
    }
    out
}

impl Combat {
    pub fn current(&self) -> Option<&Combatant> {
        self.order.get(self.turn).and_then(|id| self.units.iter().find(|u| &u.id == id))
    }
    fn idx(&self, id: &str) -> usize {
        self.units.iter().position(|u| u.id == id).unwrap_or(0)
    }
    fn cur_idx(&self) -> usize {
        self.idx(&self.order[self.turn])
    }
    /// 当前行动玩家（[`rpg::actor`]）的参战单位。
    fn player_idx(&self) -> Option<usize> {
        let me = rpg::actor();
        self.units.iter().position(|u| u.actor.as_deref() == Some(me.as_str()))
    }
    /// 轮到某个玩家角色（单人就是玩家自己；联机时是任意一名玩家）。
    pub fn players_turn(&self) -> bool {
        self.status == "active" && self.current().is_some_and(|u| u.actor.as_deref().is_some_and(rpg::is_player_actor))
    }
    /// 现在轮到的玩家角色（联机时用来判断"是不是轮到你"）。
    pub fn current_player(&self) -> Option<String> {
        if self.status != "active" {
            return None;
        }
        self.current().and_then(|u| u.actor.clone()).filter(|a| rpg::is_player_actor(a))
    }
    fn occupied(&self, x: i64, y: i64, except: usize) -> bool {
        self.obstacles.contains(&(x, y)) || self.units.iter().enumerate().any(|(i, u)| i != except && u.active() && u.x == x && u.y == y)
    }
    /// 目标是否有掩体：远程攻击时，目标旁边有障碍物、并且障碍物比目标离攻击者更近。
    fn cover(&self, ai: usize, ti: usize) -> bool {
        let (a, t) = (&self.units[ai], &self.units[ti]);
        if a.range <= 1 {
            return false;
        }
        let d_at = dist(a, t);
        self.obstacles.iter().any(|&(x, y)| (x - t.x).abs().max((y - t.y).abs()) == 1 && (x - a.x).abs().max((y - a.y).abs()) < d_at)
    }
    fn push(&mut self, lines: &mut Vec<String>, s: String) {
        lines.push(s.clone());
        self.log.push(s);
        if self.log.len() > MAX_LOG {
            let n = self.log.len() - MAX_LOG;
            self.log.drain(..n);
        }
    }
}

pub fn dist(a: &Combatant, b: &Combatant) -> i64 {
    (a.x - b.x).abs().max((a.y - b.y).abs())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EnemySpec {
    pub name: String,
    /// 对应的人物节点（有角色卡的 NPC）。
    #[serde(default)]
    pub npc: Option<String>,
    /// 没有对应人物时按等级生成数值。
    #[serde(default)]
    pub level: i64,
    #[serde(default = "one")]
    pub count: i64,
    #[serde(default)]
    pub loot: Vec<RecipeInput>,
    /// 首领（首领战无法逃跑）。
    #[serde(default)]
    pub boss: bool,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CombatAction {
    /// attack / move / defend / dodge / skill / use_item / interact / custom / flee / end_turn（interact 的 target 填物件 ID 或名字；skill 的 custom 填技能名）
    pub kind: String,
    /// 目标参战者（ID 或名字）。移动时可以填要靠近的目标。
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub x: Option<i64>,
    #[serde(default)]
    pub y: Option<i64>,
    /// 自定义行动的名称。
    #[serde(default)]
    pub custom: String,
    /// 攻击部位：head（头，命中 −3，伤害 +50%）/ arms（手臂，命中 −2，打中后对方检定 −2 三轮）/ legs（腿，命中 −1，打中后对方防御 −1、检定 −1 三轮）/ ""（不瞄准）
    #[serde(default)]
    pub part: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CombatReport {
    pub lines: Vec<String>,
    pub combat: Combat,
    /// 这一步里所有的掷骰（先攻、攻击、伤害、士气……），界面逐个演出。
    #[serde(default)]
    pub dice: Vec<campaign::DiceRecord>,
}

pub fn active(conn: &Connection, campaign_id: &str) -> Option<Combat> {
    let view = CampaignView { conn, campaign_id };
    let id = view.var(ACTIVE_VAR)?.as_str()?.to_string();
    view.record("combat", &id).and_then(|v| serde_json::from_value(v).ok())
}

pub fn get(conn: &Connection, campaign_id: &str, id: &str) -> Option<Combat> {
    (CampaignView { conn, campaign_id }).record("combat", id).and_then(|v| serde_json::from_value(v).ok())
}

/// 最近的战斗（含已结束的），新的在前。
pub fn history(conn: &Connection, campaign_id: &str, limit: usize) -> Result<Vec<Combat>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'combat' ORDER BY id DESC LIMIT ?2")?;
    let rows = stmt.query_map(rusqlite::params![campaign_id, limit as i64], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok().and_then(|d| serde_json::from_str(&d).ok())).collect())
}

fn roll(world: &mut World, campaign_id: &str, expr: &str, reason: &str) -> Result<i64> {
    Ok(campaign::roll(world, campaign_id, expr, reason)?.total)
}

fn unit_from_sheet(conn: &Connection, campaign_id: &str, s: &Sheet, side: &str, id: &str) -> Combatant {
    let (weapon, damage, range, on_hit) = rpg::weapon(conn, campaign_id, s);
    let ammo = rpg::equipped_items(conn, campaign_id, &s.actor).into_iter().find(|i| i.slot == "weapon" && !i.damage.is_empty()).map(|i| i.ammo).unwrap_or_default();
    // 只认世界或本战役里确实存在的弹药物品（旧存档里可能有模型乱写的弹药名）。
    let ammo = if ammo.is_empty() || ammo_exists(conn, campaign_id, &ammo) { ammo } else { String::new() };
    let r = rpg::rules(conn, campaign_id);
    let attr = if range > 1 { rpg::role_attr(&r, "agility") } else { rpg::role_attr(&r, "might") };
    let attr = attr.as_str();
    let wound_pen: i64 = s.wounds.iter().map(|w| w.penalty).sum();
    Combatant {
        id: id.into(),
        actor: Some(s.actor.clone()),
        name: s.name.clone(),
        side: side.into(),
        hp: s.hp,
        max_hp: s.max_hp,
        ac: rpg::armor_class(conn, campaign_id, s) - s.statuses.iter().map(|x| x.ac).sum::<i64>(),
        attack: s.attack + s.modv(attr) - wound_pen,
        weapon,
        damage: format_damage(&damage, s.modv(attr)),
        range,
        on_hit,
        ap: 0,
        init: 0,
        x: 0,
        y: 0,
        statuses: s.statuses.iter().filter(|x| x.rounds.is_none()).cloned().collect(),
        out: if s.hp > 0 { String::new() } else { "down".into() },
        level: s.level,
        xp: s.xp_value,
        loot: vec![],
        new_wounds: vec![],
        morale_checked: false,
        boss: false,
        ammo,
        speed: 0,
        dodging: false,
    }
}

fn ammo_exists(conn: &Connection, campaign_id: &str, name: &str) -> bool {
    conn.query_row("SELECT COUNT(*) FROM campaign_records WHERE campaign_id = ?1 AND kind = 'item_def' AND json_extract(data, '$.title') = ?2", rusqlite::params![campaign_id, name], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
        || conn.query_row("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL AND kind = 'item' AND title = ?1", [name], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
}

fn format_damage(d: &str, m: i64) -> String {
    match m {
        0 => d.to_string(),
        m if m > 0 => format!("{d}+{m}"),
        m => format!("{d}{m}"),
    }
}

/// 开始战斗：玩家、同伴（人物 ID）与敌人。先攻由程序掷骰。
pub fn start(world: &mut World, campaign_id: &str, enemies: &[EnemySpec], allies: &[String], reason: &str) -> Result<CombatReport> {
    start_locked(world, campaign_id, enemies, allies, reason, "")
}

/// 开始战斗；`no_escape` 非空时这场战斗无法逃跑（场地封锁、退路被切断……），写明原因。
pub fn start_locked(world: &mut World, campaign_id: &str, enemies: &[EnemySpec], allies: &[String], reason: &str, no_escape: &str) -> Result<CombatReport> {
    with_dice(|| start_inner(world, campaign_id, enemies, allies, reason, no_escape))
}

/// 执行一步战斗并把期间的掷骰记录放进报告。
fn with_dice(f: impl FnOnce() -> Result<CombatReport>) -> Result<CombatReport> {
    let (res, dice) = campaign::capture_rolls(f);
    res.map(|mut r| {
        r.dice = dice;
        r
    })
}

fn start_inner(world: &mut World, campaign_id: &str, enemies: &[EnemySpec], allies: &[String], reason: &str, no_escape: &str) -> Result<CombatReport> {
    campaign::ensure_active(world.conn(), campaign_id)?;
    if active(world.conn(), campaign_id).is_some() {
        return Err(CoreError::Invalid("已经在战斗中".into()));
    }
    if enemies.is_empty() {
        return Err(CoreError::Invalid("至少要有一个敌人".into()));
    }
    let conn = world.conn();
    let mut units = vec![];
    // 玩家方：当前行动的玩家，加上联机房间里其他没倒下的玩家（每人控制自己的单位）。
    for (k, member) in rpg::party_members(conn, campaign_id).iter().enumerate() {
        let ps = rpg::sheet(conn, campaign_id, member)?;
        if ps.hp <= 0 {
            if k == 0 {
                return Err(CoreError::Invalid(format!("{}已经倒下，不能参加战斗（先休息恢复）", ps.name)));
            }
            continue;
        }
        let id = if k == 0 { "p".to_string() } else { format!("p{}", k + 1) };
        units.push(unit_from_sheet(conn, campaign_id, &ps, "party", &id));
    }
    for (i, a) in allies.iter().take(4).enumerate() {
        let s = rpg::sheet(conn, campaign_id, a)?;
        if s.hp > 0 {
            units.push(unit_from_sheet(conn, campaign_id, &s, "ally", &format!("a{}", i + 1)));
        }
    }
    let mut n = 0;
    for e in enemies.iter().take(8) {
        let count = e.count.clamp(1, 8);
        for k in 0..count {
            n += 1;
            if n > 12 {
                break;
            }
            let name = if count > 1 { format!("{}{}", e.name.trim(), k + 1) } else { e.name.trim().to_string() };
            let mut u = match e.npc.as_deref().filter(|s| !s.is_empty()) {
                Some(npc) => {
                    let s = rpg::sheet(conn, campaign_id, npc)?;
                    unit_from_sheet(conn, campaign_id, &s, "enemy", &format!("e{n}"))
                }
                None => {
                    let s = rpg::template("", &name, rpg::difficulty_level(&rpg::rules(conn, campaign_id), e.level.max(1)));
                    let mut u = unit_from_sheet(conn, campaign_id, &s, "enemy", &format!("e{n}"));
                    u.actor = None;
                    u.damage = s.unarmed.clone();
                    u.weapon = "武器".into();
                    // 名字表明是远程的敌人（弓手、法师……）：射程 6。
                    if ["弓", "射手", "弩", "法师", "术士", "巫师", "投石"].iter().any(|w| name.contains(w)) {
                        u.range = 6;
                        u.weapon = if name.contains("法") || name.contains("术") || name.contains("巫") { "法术".into() } else { "弓箭".into() };
                    }
                    u
                }
            };
            if e.npc.is_none() {
                u.name = name;
            }
            u.loot = if k == 0 { e.loot.clone() } else { vec![] };
            u.boss = e.boss;
            units.push(u);
        }
    }
    let r = rpg::rules(conn, campaign_id);
    let (width, height) = (10, 6);
    // 站位：我方在左，敌方在右，上下错开。
    let (mut fy, mut ey) = (0, 0);
    for u in units.iter_mut() {
        if u.friendly() {
            u.x = 1 + fy / height;
            u.y = (height / 2 - 1 + if fy % 2 == 0 { fy / 2 } else { -(fy / 2) - 1 }).rem_euclid(height);
            fy += 1;
        } else {
            u.x = width - 2 - ey / height;
            u.y = (height / 2 - 1 + if ey % 2 == 0 { ey / 2 } else { -(ey / 2) - 1 }).rem_euclid(height);
            ey += 1;
        }
    }
    let mut lines = vec![];
    let agi = rpg::role_attr(&rpg::rules(world.conn(), campaign_id), "agility");
    for u in units.iter_mut() {
        // 速度 = 敏捷修正（临时敌人按等级）；先攻 = 1d20 + 速度，同分时速度高的先。
        let dex = u.actor.as_ref().and_then(|a| rpg::sheet(world.conn(), campaign_id, a).ok()).map(|s| s.modv(&agi)).unwrap_or(u.level / 3);
        u.speed = dex;
        let rv = roll(world, campaign_id, "1d20", &format!("{} 先攻", u.name))?;
        u.init = rv + dex;
        campaign::annotate_last_roll("initiative", vec![("速度".into(), dex)], u.init, None, &format!("先攻 {}", u.init));
    }
    let mut order: Vec<&Combatant> = units.iter().collect();
    order.sort_by_key(|u| (std::cmp::Reverse(u.init), std::cmp::Reverse(u.speed), u.side != "party", u.id.clone()));
    let order: Vec<String> = order.into_iter().map(|u| u.id.clone()).collect();
    let id = format!("c{:06}", history(world.conn(), campaign_id, 10_000)?.len() + 1);
    // 障碍物：3–5 个，放在战场中间几列，由战役种子和战斗编号决定（可复现），不压在任何人身上。
    let seed = campaign::get_campaign(world.conn(), campaign_id)?.rng_seed as u64;
    let mut obstacles = vec![];
    let want = 3 + (crate::worldsim::hash64(seed ^ crate::worldsim::hash64(id.len() as u64 + history(world.conn(), campaign_id, 10_000)?.len() as u64)) % 3) as usize;
    let mut k = 0u64;
    while obstacles.len() < want && k < 200 {
        k += 1;
        let h = crate::worldsim::hash64(seed.wrapping_add(k).wrapping_mul(0x9E37) ^ (id.bytes().map(|b| b as u64).sum::<u64>() << 8));
        let (x, y) = (3 + (h % (width as u64 - 6)) as i64, ((h >> 16) % height as u64) as i64);
        if !obstacles.contains(&(x, y)) && !units.iter().any(|u| u.x == x && u.y == y) {
            obstacles.push((x, y));
        }
    }
    // 场景物件：1–3 个，同样由种子决定，放在空格子上。
    let mut props: Vec<Prop> = vec![];
    let want_props = 1 + (crate::worldsim::hash64(seed ^ 0x5eed ^ id.bytes().map(|b| b as u64).sum::<u64>()) % 3) as usize;
    let mut k = 0u64;
    while props.len() < want_props && k < 300 {
        k += 1;
        let h = crate::worldsim::hash64(seed.wrapping_add(k * 7919) ^ 0xC0FFEE ^ (id.len() as u64));
        let (x, y) = (2 + (h % (width as u64 - 4)) as i64, ((h >> 20) % height as u64) as i64);
        let kind = PROP_KINDS[((h >> 40) % PROP_KINDS.len() as u64) as usize];
        if obstacles.contains(&(x, y)) || units.iter().any(|u| u.x == x && u.y == y) || props.iter().any(|p| (p.x, p.y) == (x, y) || p.kind == kind.0) {
            continue;
        }
        props.push(Prop { id: format!("o{}", props.len() + 1), name: kind.1.into(), kind: kind.0.into(), x, y, used: false, desc: kind.2.into() });
    }
    let mut c = Combat { id, round: 1, turn: 0, order, units, log: vec![], status: "active".into(), width, height, reason: reason.into(), rewards: vec![], obstacles, no_escape: no_escape.trim().chars().take(60).collect(), props };
    let order_text = c.order.iter().map(|o| { let u = &c.units[c.idx(o)]; format!("{}({})", u.name, u.init) }).collect::<Vec<_>>().join(" → ");
    c.push(&mut lines, format!("战斗开始（{reason}）。先攻顺序：{order_text}"));
    begin_turn(world, campaign_id, &mut c, &r, &mut lines)?;
    run_ai(world, campaign_id, &mut c, &r, &mut lines)?;
    save(world, campaign_id, &mut c, &mut lines)?;
    Ok(CombatReport { lines, combat: c, dice: vec![] })
}

/// 回合开始：恢复行动点，处理持续伤害与状态到期；眩晕时跳过。返回是否可以行动。
fn begin_turn(world: &mut World, campaign_id: &str, c: &mut Combat, r: &RuleSet, lines: &mut Vec<String>) -> Result<bool> {
    let i = c.cur_idx();
    if !c.units[i].active() {
        return Ok(false);
    }
    c.units[i].ap = r.ap_per_turn;
    c.units[i].dodging = false;
    let mut skip = false;
    let statuses = c.units[i].statuses.clone();
    for s in &statuses {
        if !s.dot.is_empty() && c.units[i].active() {
            let d = roll(world, campaign_id, &s.dot, &format!("{} 受到{}伤害", c.units[i].name, s.name))?.max(1);
            c.units[i].hp -= d;
            let name = c.units[i].name.clone();
            c.push(lines, format!("{name}受到{} {d} 点伤害（生命 {}）", s.name, c.units[i].hp));
            knock_out(c, i, lines, now(world.conn(), campaign_id));
        }
        if s.skip {
            skip = true;
        }
    }
    for s in c.units[i].statuses.iter_mut() {
        if let Some(n) = s.rounds.as_mut() {
            *n -= 1;
        }
    }
    let expired: Vec<String> = c.units[i].statuses.iter().filter(|s| s.rounds.is_some_and(|n| n <= 0)).map(|s| s.name.clone()).collect();
    c.units[i].statuses.retain(|s| s.rounds.is_none_or(|n| n > 0));
    if !expired.is_empty() {
        let name = c.units[i].name.clone();
        c.push(lines, format!("{name}的{}结束了", expired.join("、")));
    }
    if skip && c.units[i].active() {
        let name = c.units[i].name.clone();
        c.push(lines, format!("{name}无法行动，跳过这一回合"));
        return Ok(false);
    }
    Ok(c.units[i].active())
}

fn now(conn: &Connection, campaign_id: &str) -> i64 {
    crate::schedule::clock(conn, campaign_id)
}

/// 生命值降到 0：倒下，有角色卡的记一处重伤。
fn knock_out(c: &mut Combat, i: usize, lines: &mut Vec<String>, minutes: i64) {
    if c.units[i].hp > 0 || !c.units[i].active() {
        return;
    }
    let dead = c.units[i].actor.is_some() && c.units[i].hp <= -(c.units[i].max_hp / 2).max(1) && !c.units[i].actor.as_deref().is_some_and(rpg::is_player_actor);
    c.units[i].out = if dead { "dead".into() } else { "down".into() };
    if c.units[i].actor.is_some() && !dead {
        c.units[i].new_wounds.push(Wound { name: "重伤".into(), penalty: 2, attr: String::new(), heal_at: minutes + 3 * 1440, source: c.reason.clone() });
    }
    let name = c.units[i].name.clone();
    c.push(lines, format!("{name}{}", if dead { "死了" } else { "倒下了" }));
}

/// 结束当前回合，轮到下一个还在战斗的人。
fn next_turn(world: &mut World, campaign_id: &str, c: &mut Combat, r: &RuleSet, lines: &mut Vec<String>) -> Result<()> {
    for _ in 0..c.order.len() * 2 {
        c.turn += 1;
        if c.turn >= c.order.len() {
            c.turn = 0;
            c.round += 1;
        }
        if c.units[c.cur_idx()].active() {
            if begin_turn(world, campaign_id, c, r, lines)? {
                return Ok(());
            }
            if check_end(c, lines) {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn check_end(c: &mut Combat, lines: &mut Vec<String>) -> bool {
    if c.status != "active" {
        return true;
    }
    let enemies = c.units.iter().any(|u| !u.friendly() && u.active());
    let friends = c.units.iter().any(|u| u.friendly() && u.active());
    if !enemies {
        c.status = "victory".into();
        c.push(lines, "敌人全部倒下或逃走，战斗胜利".into());
    } else if !friends {
        c.status = "defeat".into();
        c.push(lines, "我方全部倒下，战斗失败".into());
    }
    c.status != "active"
}

/// 攻击：命中检定、伤害、暴击、命中附带状态。
fn attack(world: &mut World, campaign_id: &str, c: &mut Combat, r: &RuleSet, ai: usize, ti: usize, lines: &mut Vec<String>) -> Result<()> {
    let (an, tn) = (c.units[ai].name.clone(), c.units[ti].name.clone());
    // 用弹药的武器：先扣一发；没有弹药就打不响（这一击作废）。
    if let (false, Some(actor)) = (c.units[ai].ammo.is_empty(), c.units[ai].actor.clone()) {
        let ammo = c.units[ai].ammo.clone();
        match rpg::ammo_stack(world.conn(), campaign_id, &actor, &ammo) {
            Some((stack, n)) => {
                campaign::apply_effects(world, campaign_id, &[Effect::TakeItem { holder: actor.clone(), item: stack, qty: 1 }], &format!("{an}射击，消耗 1 发{ammo}"))?;
                c.push(lines, format!("{an}消耗 1 发{ammo}（剩 {}）", n - 1));
            }
            None => {
                let weapon = c.units[ai].weapon.clone();
                c.push(lines, format!("{an}扣下{weapon}的扳机——没有{ammo}了，打不响"));
                return Ok(());
            }
        }
    }
    let n = roll(world, campaign_id, "1d20", &format!("{an} 攻击 {tn}"))?;
    let bonus = c.units[ai].attack + c.units[ai].mods();
    let cover = c.cover(ai, ti);
    let ac = c.units[ti].ac_now() + if cover { 2 } else { 0 };
    let ac_note = if cover { "（掩体 +2）" } else { "" };
    let crit = n >= r.crit_on;
    let hit = n != 1 && (crit || n + bonus >= ac);
    {
        let mut parts = vec![("攻击加值".to_string(), c.units[ai].attack)];
        if c.units[ai].mods() != 0 {
            parts.push(("状态".into(), c.units[ai].mods()));
        }
        let outcome = if n == 1 { "大失手" } else if crit { "暴击" } else if hit { "命中" } else { "没有命中" };
        campaign::annotate_last_roll("attack", parts, n + bonus, Some(ac), outcome);
    }
    let weapon = c.units[ai].weapon.clone();
    if !hit {
        c.push(lines, format!("{an}用{weapon}攻击{tn}：掷出 {n}{bonus:+}={}，防御 {ac}{ac_note}，{}", n + bonus, if n == 1 { "大失手" } else { "没有命中" }));
        return Ok(());
    }
    // 目标在闪避：闪避检定（1d20 + 速度）不低于攻击总值就躲开（暴击躲不开）。
    if c.units[ti].dodging && !crit {
        let sp = c.units[ti].speed;
        let dn = roll(world, campaign_id, "1d20", &format!("{tn} 闪避 {an}"))?;
        let ok = dn + sp >= n + bonus;
        campaign::annotate_last_roll("dodge", vec![("速度".into(), sp)], dn + sp, Some(n + bonus), if ok { "闪开" } else { "没闪开" });
        if ok {
            c.push(lines, format!("{an}用{weapon}攻击{tn}：掷出 {n}{bonus:+}={}，命中——但{tn}闪避（{dn}{sp:+}={}）躲开了", n + bonus, dn + sp));
            return Ok(());
        }
        c.push(lines, format!("{tn}想闪避（{dn}{sp:+}={}），没能躲开", dn + sp));
    }
    let dmg_expr = c.units[ai].damage.clone();
    let mut dmg = roll(world, campaign_id, &dmg_expr, &format!("{an} 对 {tn} 的伤害"))?;
    campaign::annotate_last_roll("damage", vec![], dmg, None, &format!("{dmg} 点伤害"));
    if crit {
        let extra = roll(world, campaign_id, &dmg_expr, &format!("{an} 暴击追加伤害"))?;
        campaign::annotate_last_roll("damage", vec![], extra, None, &format!("暴击追加 {extra} 点"));
        dmg += extra;
    }
    let dmg = dmg.max(1);
    c.units[ti].hp -= dmg;
    c.push(
        lines,
        format!("{an}用{weapon}攻击{tn}：掷出 {n}{bonus:+}={}，防御 {ac}{ac_note}，{}命中，造成 {dmg} 点伤害（{tn} 生命 {}/{}）", n + bonus, if crit { "暴击！" } else { "" }, c.units[ti].hp.max(0), c.units[ti].max_hp),
    );
    let minutes = now(world.conn(), campaign_id);
    if crit && c.units[ti].actor.is_some() && c.units[ti].hp > 0 {
        c.units[ti].new_wounds.push(Wound { name: "重创".into(), penalty: 1, attr: String::new(), heal_at: minutes + 1440, source: format!("被{an}暴击") });
    }
    if let Some(st) = c.units[ai].on_hit.clone()
        && c.units[ti].hp > 0
    {
        apply_status(c, ti, st, &an, lines);
    }
    knock_out(c, ti, lines, minutes);
    Ok(())
}

fn apply_status(c: &mut Combat, ti: usize, mut st: Status, from: &str, lines: &mut Vec<String>) {
    st.source = from.to_string();
    if st.rounds.is_none() {
        st.rounds = Some(2);
    }
    let tn = c.units[ti].name.clone();
    c.units[ti].statuses.retain(|s| s.name != st.name);
    c.push(lines, format!("{tn}陷入{}（{} 轮）", st.name, st.rounds.unwrap_or(0)));
    c.units[ti].statuses.push(st);
}

/// 向目标移动最多 steps 格，停在射程内。返回走了几格。
fn step_toward(c: &mut Combat, mi: usize, ti: usize, steps: i64, stop_at: i64) -> i64 {
    let mut moved = 0;
    while moved < steps && dist(&c.units[mi], &c.units[ti]) > stop_at {
        let (dx, dy) = ((c.units[ti].x - c.units[mi].x).signum(), (c.units[ti].y - c.units[mi].y).signum());
        let cands = [(dx, dy), (dx, 0), (0, dy), (dx, -dy), (-dy, dx)];
        let Some((nx, ny)) = cands
            .iter()
            .filter(|(a, b)| *a != 0 || *b != 0)
            .map(|(a, b)| (c.units[mi].x + a, c.units[mi].y + b))
            .find(|(x, y)| *x >= 0 && *y >= 0 && *x < c.width && *y < c.height && !c.occupied(*x, *y, mi))
        else {
            break;
        };
        c.units[mi].x = nx;
        c.units[mi].y = ny;
        moved += 1;
    }
    moved
}

fn nearest_foe(c: &Combat, i: usize) -> Option<usize> {
    let me = &c.units[i];
    c.units.iter().enumerate().filter(|(_, u)| u.active() && u.friendly() != me.friendly()).min_by_key(|(_, u)| (dist(me, u), u.hp)).map(|(j, _)| j)
}

/// 选目标：射程内生命最少的对手；射程内没有就选最近的。
fn pick_target(c: &Combat, i: usize) -> Option<usize> {
    let me = &c.units[i];
    c.units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.active() && u.friendly() != me.friendly() && dist(me, u) <= me.range)
        .min_by_key(|(_, u)| (u.hp, dist(me, u)))
        .map(|(j, _)| j)
        .or_else(|| nearest_foe(c, i))
}

/// 远离对手最多 steps 格（远程单位拉开距离），返回走了几格。
fn step_away(c: &mut Combat, mi: usize, ti: usize, steps: i64) -> i64 {
    let mut moved = 0;
    while moved < steps {
        let cur = dist(&c.units[mi], &c.units[ti]);
        let (x0, y0) = (c.units[mi].x, c.units[mi].y);
        let best = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)]
            .iter()
            .map(|(a, b)| (x0 + a, y0 + b))
            .filter(|(x, y)| *x >= 0 && *y >= 0 && *x < c.width && *y < c.height && !c.occupied(*x, *y, mi))
            .max_by_key(|(x, y)| (*x - c.units[ti].x).abs().max((*y - c.units[ti].y).abs()));
        match best {
            Some((x, y)) if (x - c.units[ti].x).abs().max((y - c.units[ti].y).abs()) > cur => {
                c.units[mi].x = x;
                c.units[mi].y = y;
                moved += 1;
            }
            _ => break,
        }
    }
    moved
}

/// 程序控制的参战者：靠近最近的对手并攻击。
fn ai_turn(world: &mut World, campaign_id: &str, c: &mut Combat, r: &RuleSet, lines: &mut Vec<String>) -> Result<()> {
    let i = c.cur_idx();
    // 士气：敌人的生命降到四分之一时检定一次，失败就逃跑（逃走的敌人不给经验）。
    if c.units[i].side == "enemy" && !c.units[i].morale_checked && c.units[i].hp * 4 <= c.units[i].max_hp && c.units[i].active() {
        c.units[i].morale_checked = true;
        let n = c.units[i].name.clone();
        let rv = roll(world, campaign_id, "1d20", &format!("{n} 士气"))?;
        let total = rv + c.units[i].level / 2;
        if total < 10 {
            c.units[i].out = "fled".into();
            c.push(lines, format!("{n}伤势过重，士气崩溃（{rv}+{}={total}，难度 10），落荒而逃", c.units[i].level / 2));
            check_end(c, lines);
            return Ok(());
        }
        c.push(lines, format!("{n}负伤但咬牙坚持（士气 {total}）"));
    }
    for _ in 0..10 {
        if !c.units[i].active() || c.status != "active" {
            break;
        }
        let Some(t) = pick_target(c, i) else { break };
        let d = dist(&c.units[i], &c.units[t]);
        // 远程单位被贴身时先拉开距离（行动点够拉开再射击时）。
        if c.units[i].range > 1 && d <= 1 && c.units[i].ap >= r.move_cost.max(1) + r.attack_cost {
            let moved = step_away(c, i, t, 2);
            if moved > 0 {
                c.units[i].ap -= r.move_cost.max(1);
                let (n, tn) = (c.units[i].name.clone(), c.units[t].name.clone());
                c.push(lines, format!("{n}后退 {moved} 格，和{tn}拉开距离"));
                continue;
            }
        }
        if d <= c.units[i].range && c.units[i].ap >= r.attack_cost {
            c.units[i].ap -= r.attack_cost;
            attack(world, campaign_id, c, r, i, t, lines)?;
            if check_end(c, lines) {
                break;
            }
        } else if d > c.units[i].range && c.units[i].ap >= r.move_cost.max(1) {
            let range = c.units[i].range;
            let moved = step_toward(c, i, t, r.speed, range);
            if moved == 0 {
                break;
            }
            c.units[i].ap -= r.move_cost.max(1);
            let (n, tn) = (c.units[i].name.clone(), c.units[t].name.clone());
            c.push(lines, format!("{n}向{tn}移动了 {moved} 格"));
        } else {
            break;
        }
    }
    Ok(())
}

/// 轮到玩家之前，程序控制的参战者依次行动。
fn run_ai(world: &mut World, campaign_id: &str, c: &mut Combat, r: &RuleSet, lines: &mut Vec<String>) -> Result<()> {
    for _ in 0..200 {
        if check_end(c, lines) || c.players_turn() {
            break;
        }
        let i = c.cur_idx();
        if c.units[i].active() && !c.units[i].actor.as_deref().is_some_and(rpg::is_player_actor) {
            ai_turn(world, campaign_id, c, r, lines)?;
            if check_end(c, lines) {
                break;
            }
        }
        next_turn(world, campaign_id, c, r, lines)?;
    }
    Ok(())
}

fn find_unit(c: &Combat, key: &str) -> Option<usize> {
    let k = key.trim();
    if k.is_empty() {
        return None;
    }
    c.units.iter().position(|u| u.id == k || u.name == k).or_else(|| c.units.iter().position(|u| u.active() && (u.name.contains(k) || k.contains(&u.name))))
}

/// 玩家在自己的回合行动。行动点用完或结束回合后，其他参战者由程序行动，直到再次轮到玩家或战斗结束。
pub fn act(world: &mut World, campaign_id: &str, a: &CombatAction) -> Result<CombatReport> {
    with_dice(|| act_inner(world, campaign_id, a))
}

fn act_inner(world: &mut World, campaign_id: &str, a: &CombatAction) -> Result<CombatReport> {
    let mut c = active(world.conn(), campaign_id).ok_or_else(|| CoreError::Invalid("现在没有在战斗".into()))?;
    if !c.players_turn() {
        return Err(CoreError::Invalid("还没轮到玩家行动".into()));
    }
    if let Some(who) = c.current_player()
        && who != rpg::actor()
    {
        let name = c.current().map(|u| u.name.clone()).unwrap_or_default();
        return Err(CoreError::Invalid(format!("现在轮到{name}行动")));
    }
    let r = rpg::rules(world.conn(), campaign_id);
    let pi = c.player_idx().unwrap_or(0);
    let mut lines = vec![];
    let need = |cost: i64, c: &Combat| -> Result<()> {
        if c.units[pi].ap < cost {
            Err(CoreError::Invalid(format!("行动点不够（剩 {}，需要 {cost}）", c.units[pi].ap)))
        } else {
            Ok(())
        }
    };
    let mut end = false;
    let mut fled = false;
    match a.kind.as_str() {
        "attack" => {
            need(r.attack_cost, &c)?;
            let t = find_unit(&c, &a.target).filter(|t| !c.units[*t].friendly() && c.units[*t].active()).ok_or_else(|| CoreError::Invalid(format!("找不到可以攻击的目标：{}", a.target)))?;
            let d = dist(&c.units[pi], &c.units[t]);
            if d > c.units[pi].range {
                return Err(CoreError::Invalid(format!("{}太远了（距离 {d} 格，射程 {}），先移动过去", c.units[t].name, c.units[pi].range)));
            }
            c.units[pi].ap -= r.attack_cost;
            // 瞄准部位：先加命中修正，打中后按部位附加效果（伤害 / 状态）。
            let (aim, part_name) = match a.part.as_str() {
                "head" => (-3, "头部"),
                "arms" => (-2, "手臂"),
                "legs" => (-1, "腿"),
                _ => (0, ""),
            };
            if aim != 0 {
                c.units[pi].statuses.push(Status { name: "瞄准部位".into(), rounds: Some(1), modifier: aim, ..Default::default() });
            }
            let hp_before = c.units[t].hp;
            attack(world, campaign_id, &mut c, &r, pi, t, &mut lines)?;
            c.units[pi].statuses.retain(|s| s.name != "瞄准部位");
            let dealt = hp_before - c.units[t].hp;
            if aim != 0 && dealt > 0 && c.units[t].hp > 0 {
                let (pn, tn) = (c.units[pi].name.clone(), c.units[t].name.clone());
                match a.part.as_str() {
                    "head" => {
                        let extra = (dealt + 1) / 2;
                        c.units[t].hp -= extra;
                        c.push(&mut lines, format!("{pn}打中了{tn}的{part_name}，额外造成 {extra} 点伤害（{tn} 生命 {}/{}）", c.units[t].hp.max(0), c.units[t].max_hp));
                        let m = now(world.conn(), campaign_id);
                        knock_out(&mut c, t, &mut lines, m);
                    }
                    "arms" => apply_status(&mut c, t, Status { name: "手臂受伤".into(), rounds: Some(3), modifier: -2, ..Default::default() }, &pn, &mut lines),
                    _ => apply_status(&mut c, t, Status { name: "腿伤".into(), rounds: Some(3), modifier: -1, ac: -1, ..Default::default() }, &pn, &mut lines),
                }
            } else if aim != 0 && dealt <= 0 {
                let pn = c.units[pi].name.clone();
                c.push(&mut lines, format!("（{pn}瞄准的是{part_name}）"));
            }
        }
        "destroy" => {
            // 破坏环境：砸毁掩体（站在后面的人失去掩护）。
            need(r.attack_cost, &c)?;
            let pidx = c.props.iter().position(|p| (p.id == a.target || p.name == a.target.trim()) && !p.used).ok_or_else(|| CoreError::Invalid(format!("战场上没有能破坏的物件：{}", a.target)))?;
            let prop = c.props[pidx].clone();
            let d = (prop.x - c.units[pi].x).abs().max((prop.y - c.units[pi].y).abs());
            if d > c.units[pi].range.max(1) {
                return Err(CoreError::Invalid(format!("「{}」太远（距离 {d} 格）", prop.name)));
            }
            c.units[pi].ap -= r.attack_cost;
            c.props[pidx].used = true;
            let pn = c.units[pi].name.clone();
            c.push(&mut lines, format!("{pn}砸毁了{}", prop.name));
            for u in c.units.iter_mut().filter(|u| u.active() && (u.x - prop.x).abs().max((u.y - prop.y).abs()) <= 1) {
                u.statuses.retain(|s| s.name != "掩护");
            }
        }
        "surrender" => {
            // 投降：战斗结束，接下来怎么样由对方（GM 按人物和世界规则）决定。
            let name = c.units[pi].name.clone();
            c.status = "ended".into();
            c.reason = format!("{name}投降了");
            c.push(&mut lines, format!("{name}放下武器投降了，接下来怎么样由对方决定"));
        }
        "move" => {
            need(r.move_cost, &c)?;
            let moved = if let (Some(x), Some(y)) = (a.x, a.y) {
                let d = (x - c.units[pi].x).abs().max((y - c.units[pi].y).abs());
                if x < 0 || y < 0 || x >= c.width || y >= c.height {
                    return Err(CoreError::Invalid("目标位置在战场外".into()));
                }
                if d > r.speed {
                    return Err(CoreError::Invalid(format!("一次最多移动 {} 格", r.speed)));
                }
                if c.occupied(x, y, pi) {
                    return Err(CoreError::Invalid("那里有人或障碍物".into()));
                }
                c.units[pi].x = x;
                c.units[pi].y = y;
                d
            } else {
                let t = find_unit(&c, &a.target).ok_or_else(|| CoreError::Invalid("移动需要目标位置或要靠近的目标".into()))?;
                let stop = if c.units[t].friendly() { 1 } else { c.units[pi].range };
                let (ox, oy) = (c.units[pi].x, c.units[pi].y);
                step_toward(&mut c, pi, t, r.speed, stop);
                // 按实际走了多远报告（实测：被挡住时原地不动却报告"移动了 4 格"，还会重复两次）。
                let d = (c.units[pi].x - ox).abs().max((c.units[pi].y - oy).abs());
                if d == 0 {
                    return Err(CoreError::Invalid("没法再靠近了（路被挡住或已经在旁边）".into()));
                }
                d
            };
            c.units[pi].ap -= r.move_cost;
            let (n, x, y) = (c.units[pi].name.clone(), c.units[pi].x, c.units[pi].y);
            c.push(&mut lines, format!("{n}移动了 {moved} 格（到 {x},{y}）"));
        }
        "dodge" => {
            need(1, &c)?;
            c.units[pi].ap -= 1;
            c.units[pi].dodging = true;
            let (n, sp) = (c.units[pi].name.clone(), c.units[pi].speed);
            c.push(&mut lines, format!("{n}准备闪避（到下次行动前，被攻击时掷 1d20{sp:+} 对抗攻击）"));
            end = true;
        }
        "interact" => {
            need(1, &c)?;
            let pidx = c.props.iter().position(|p| p.id == a.target || p.name == a.target.trim()).ok_or_else(|| CoreError::Invalid(format!("战场上没有这个物件：{}", a.target)))?;
            let prop = c.props[pidx].clone();
            if prop.used {
                return Err(CoreError::Invalid(format!("「{}」已经用过了", prop.name)));
            }
            let d = (prop.x - c.units[pi].x).abs().max((prop.y - c.units[pi].y).abs());
            if d > 1 {
                return Err(CoreError::Invalid(format!("「{}」太远（距离 {d} 格），先走到旁边", prop.name)));
            }
            c.units[pi].ap -= 1;
            let pn = c.units[pi].name.clone();
            match prop.kind.as_str() {
                "cover" => {
                    apply_status(&mut c, pi, Status { name: "掩护".into(), rounds: Some(2), ac: 2, ..Default::default() }, &prop.name, &mut lines);
                    c.push(&mut lines, format!("{pn}躲到{}后面", prop.name));
                }
                "fire" => {
                    c.props[pidx].used = true;
                    let t = c.units.iter().enumerate().filter(|(_, u)| !u.friendly() && u.active() && (u.x - prop.x).abs().max((u.y - prop.y).abs()) <= 2).min_by_key(|(_, u)| (u.x - prop.x).abs().max((u.y - prop.y).abs())).map(|(i, _)| i);
                    match t {
                        Some(t) => {
                            let tn = c.units[t].name.clone();
                            let dmg = roll(world, campaign_id, "1d6", &format!("{pn} 打翻{}，{tn}受到的火焰伤害", prop.name))?.max(1);
                            campaign::annotate_last_roll("damage", vec![], dmg, None, &format!("{dmg} 点火焰伤害"));
                            c.units[t].hp -= dmg;
                            c.push(&mut lines, format!("{pn}打翻{}，火焰扑向{tn}，造成 {dmg} 点伤害（{tn} 生命 {}/{}）", prop.name, c.units[t].hp.max(0), c.units[t].max_hp));
                            if c.units[t].hp > 0 {
                                apply_status(&mut c, t, Status { name: "灼烧".into(), rounds: Some(2), dot: "1d2".into(), ..Default::default() }, &pn, &mut lines);
                            }
                            let m = now(world.conn(), campaign_id);
                            knock_out(&mut c, t, &mut lines, m);
                        }
                        None => c.push(&mut lines, format!("{pn}打翻{}，但周围没有敌人，火很快熄了", prop.name)),
                    }
                }
                "drop" => {
                    c.props[pidx].used = true;
                    let near: Vec<usize> = c.units.iter().enumerate().filter(|(_, u)| !u.friendly() && u.active() && (u.x - prop.x).abs().max((u.y - prop.y).abs()) <= 1).map(|(i, _)| i).collect();
                    if near.is_empty() {
                        c.push(&mut lines, format!("{pn}割断绳子，{}砸在地上，但旁边没有敌人", prop.name));
                    }
                    for t in near {
                        let (tn, sp) = (c.units[t].name.clone(), c.units[t].speed);
                        let n = roll(world, campaign_id, "1d20", &format!("{tn} 躲开砸下的{}（敏捷检定）", prop.name))?;
                        let ok = n + sp >= 12;
                        campaign::annotate_last_roll("check", vec![("速度".into(), sp)], n + sp, Some(12), if ok { "躲开" } else { "被砸中" });
                        if ok {
                            c.push(&mut lines, format!("{}砸下来，{tn}躲开了（{n}{sp:+}，难度 12）", prop.name));
                        } else {
                            c.push(&mut lines, format!("{}砸下来，{tn}没躲开（{n}{sp:+}，难度 12），倒在地上", prop.name));
                            apply_status(&mut c, t, Status { name: "倒地".into(), rounds: Some(1), skip: true, ac: -2, ..Default::default() }, &pn, &mut lines);
                        }
                    }
                }
                "mechanism" => {
                    c.props[pidx].used = true;
                    let near: Vec<usize> = c.units.iter().enumerate().filter(|(_, u)| !u.friendly() && u.active() && (u.x - prop.x).abs().max((u.y - prop.y).abs()) <= 2).map(|(i, _)| i).collect();
                    c.push(&mut lines, format!("{pn}拉动了{}", prop.name));
                    if near.is_empty() {
                        c.push(&mut lines, "机关启动了，但附近没有敌人".to_string());
                    }
                    for t in near {
                        let (tn, sp) = (c.units[t].name.clone(), c.units[t].speed);
                        let n = roll(world, campaign_id, "1d20", &format!("{tn} 躲开{}（敏捷检定）", prop.name))?;
                        let ok = n + sp >= 13;
                        campaign::annotate_last_roll("check", vec![("速度".into(), sp)], n + sp, Some(13), if ok { "躲开" } else { "被击中" });
                        if ok {
                            c.push(&mut lines, format!("{tn}躲开了机关（{n}{sp:+}，难度 13）"));
                        } else {
                            let dmg = roll(world, campaign_id, "1d8", &format!("{}对{tn}的伤害", prop.name))?.max(1);
                            campaign::annotate_last_roll("damage", vec![], dmg, None, &format!("{dmg} 点伤害"));
                            c.units[t].hp -= dmg;
                            c.push(&mut lines, format!("机关击中{tn}，造成 {dmg} 点伤害（{tn} 生命 {}/{}）", c.units[t].hp.max(0), c.units[t].max_hp));
                            let m = now(world.conn(), campaign_id);
                            knock_out(&mut c, t, &mut lines, m);
                        }
                    }
                }
                other => return Err(CoreError::Invalid(format!("不认识的物件类型：{other}"))),
            }
        }
        "defend" => {
            need(1, &c)?;
            c.units[pi].ap -= 1;
            let n = c.units[pi].name.clone();
            apply_status(&mut c, pi, Status { name: "防御姿态".into(), rounds: Some(1), ac: 2, ..Default::default() }, &n, &mut lines);
            end = true;
        }
        "use_item" => {
            need(1, &c)?;
            let it = rpg::item_info(world.conn(), &a.item)?.ok_or_else(|| CoreError::NotFound("物品", a.item.clone()))?;
            if it.heal.is_empty() && it.cures.is_empty() {
                return Err(CoreError::Invalid(format!("「{}」不能在战斗中使用", it.title)));
            }
            if (CampaignView { conn: world.conn(), campaign_id }).item_qty(&rpg::actor(), &it.id) < 1 {
                return Err(CoreError::Invalid(format!("没有「{}」", it.title)));
            }
            let mut text = format!("{}使用了「{}」", c.units[pi].name, it.title);
            if !it.heal.is_empty() {
                let h = roll(world, campaign_id, &it.heal, &format!("使用「{}」", it.title))?.max(0);
                let u = &mut c.units[pi];
                let before = u.hp;
                u.hp = (u.hp + h).min(u.max_hp);
                text.push_str(&format!("，恢复 {} 点生命（{} → {}）", u.hp - before, before, u.hp));
            }
            c.units[pi].statuses.retain(|s| !it.cures.contains(&s.name));
            c.units[pi].ap -= 1;
            campaign::apply_effects(world, campaign_id, &[Effect::TakeItem { holder: rpg::actor(), item: it.id.clone(), qty: 1 }], &text)?;
            c.push(&mut lines, text);
        }
        "custom" | "skill" => {
            let name = if a.custom.trim().is_empty() { a.item.trim() } else { a.custom.trim() };
            let ca = skills(&r).into_iter().find(|x| x.name == name).ok_or_else(|| CoreError::Invalid(format!("没有这个技能：{name}")))?;
            need(ca.ap, &c)?;
            let t = if ca.range == 0 { pi } else { find_unit(&c, &a.target).filter(|t| c.units[*t].active()).ok_or_else(|| CoreError::Invalid("需要一个目标".into()))? };
            let d = dist(&c.units[pi], &c.units[t]);
            if d > ca.range.max(0) {
                return Err(CoreError::Invalid(format!("目标太远（距离 {d}，射程 {}）", ca.range)));
            }
            c.units[pi].ap -= ca.ap;
            let bonus = c.units[pi].actor.as_ref().and_then(|x| rpg::sheet(world.conn(), campaign_id, x).ok()).map(|s| if ca.attr.is_empty() { 0 } else { s.modv(&ca.attr) }).unwrap_or(0);
            let (pn, tn) = (c.units[pi].name.clone(), c.units[t].name.clone());
            match ca.kind.as_str() {
                "heal" => {
                    let h = (roll(world, campaign_id, if ca.dice.is_empty() { "1d4" } else { &ca.dice }, &format!("{pn} 的{}", ca.name))? + bonus).max(0);
                    let u = &mut c.units[t];
                    u.hp = (u.hp + h).min(u.max_hp);
                    let hp = u.hp;
                    c.push(&mut lines, format!("{pn}使用「{}」，{tn}恢复 {h} 点生命（生命 {hp}）", ca.name));
                }
                "attack" => {
                    let n = roll(world, campaign_id, "1d20", &format!("{pn} 的{}", ca.name))?;
                    let total = n + c.units[pi].attack + c.units[pi].mods() + bonus;
                    let ac = c.units[t].ac_now();
                    if n != 1 && (n >= r.crit_on || total >= ac) {
                        let dmg = (roll(world, campaign_id, if ca.dice.is_empty() { "1d6" } else { &ca.dice }, &format!("{pn} 的{}伤害", ca.name))? + bonus).max(1);
                        c.units[t].hp -= dmg;
                        let hp = c.units[t].hp;
                        c.push(&mut lines, format!("{pn}使用「{}」命中{tn}（{total} 对防御 {ac}），造成 {dmg} 点伤害（生命 {}）", ca.name, hp.max(0)));
                        if let Some(st) = ca.status.clone() {
                            apply_status(&mut c, t, st, &pn, &mut lines);
                        }
                        let m = now(world.conn(), campaign_id);
                        knock_out(&mut c, t, &mut lines, m);
                    } else {
                        c.push(&mut lines, format!("{pn}使用「{}」，没有命中{tn}（{total} 对防御 {ac}）", ca.name));
                    }
                }
                _ => {
                    if let Some(st) = ca.status.clone() {
                        c.push(&mut lines, format!("{pn}使用「{}」", ca.name));
                        apply_status(&mut c, t, st, &pn, &mut lines);
                    }
                }
            }
        }
        "flee" if let Some(why) = escape_blocked(&c, pi) => {
            // 无法逃跑：不掷骰、不消耗行动，说明原因。
            let name = c.units[pi].name.clone();
            c.push(&mut lines, format!("{name}想逃跑，但无法脱身：{why}"));
        }
        "flee" => {
            let adj = c.units.iter().filter(|u| !u.friendly() && u.active() && dist(u, &c.units[pi]) <= 1).count() as i64;
            let dc = 10 + adj * 2;
            let dex = rpg::sheet(world.conn(), campaign_id, &rpg::actor())?.modv(&rpg::role_attr(&rpg::rules(world.conn(), campaign_id), "agility"));
            let n = roll(world, campaign_id, "1d20", "逃跑")?;
            c.units[pi].ap = 0;
            let name = c.units[pi].name.clone();
            if n + dex >= dc {
                c.units[pi].out = "fled".into();
                c.status = "fled".into();
                c.push(&mut lines, format!("{name}逃跑：掷出 {n}{dex:+}，难度 {dc}，成功脱离了战斗"));
                fled = true;
            } else {
                c.push(&mut lines, format!("{name}逃跑：掷出 {n}{dex:+}，难度 {dc}，没能脱身"));
            }
            end = true;
        }
        "end_turn" => end = true,
        other => return Err(CoreError::Invalid(format!("未知的战斗行动：{other}（可用 attack/move/defend/dodge/skill/use_item/interact/destroy/surrender/flee/end_turn）"))),
    }
    if !check_end(&mut c, &mut lines) && (end || c.units[pi].ap <= 0 || !c.units[pi].active()) {
        next_turn(world, campaign_id, &mut c, &r, &mut lines)?;
        run_ai(world, campaign_id, &mut c, &r, &mut lines)?;
    }
    save(world, campaign_id, &mut c, &mut lines)?;
    // 逃跑成功：离开当前地点，跑到最近的可到达地点（有地图时；耗时按路程和天气计算）。
    if fled
        && let Some(to) = crate::worldsim::reachable(world.conn(), campaign_id, 1)?.into_iter().next()
        && let Ok((plan, more)) = crate::worldsim::travel(world, campaign_id, &to.id, "逃离战斗")
    {
        lines.push(format!("逃到了「{}」（{}）", plan.to_title, crate::worldsim::format_duration(plan.minutes)));
        lines.extend(more);
    }
    Ok(CombatReport { lines, combat: c, dice: vec![] })
}

/// 战斗中更换装备后，按角色卡重新计算这个人的武器、伤害、射程、防御（生命、位置、状态不变）。
pub fn refresh_unit(world: &mut World, campaign_id: &str, actor: &str) -> Result<()> {
    let Some(mut c) = active(world.conn(), campaign_id) else { return Ok(()) };
    let Some(i) = c.units.iter().position(|u| u.actor.as_deref() == Some(actor)) else { return Ok(()) };
    let s = rpg::sheet(world.conn(), campaign_id, actor)?;
    let fresh = unit_from_sheet(world.conn(), campaign_id, &s, &c.units[i].side, &c.units[i].id);
    let u = &mut c.units[i];
    if (u.weapon.as_str(), u.ac) == (fresh.weapon.as_str(), fresh.ac) {
        return Ok(());
    }
    (u.weapon, u.damage, u.range, u.on_hit, u.attack, u.ac) = (fresh.weapon, fresh.damage, fresh.range, fresh.on_hit, fresh.attack, fresh.ac);
    let line = format!("{}换上了{}", u.name, u.weapon);
    c.log.push(line);
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "combat".into(), id: c.id.clone(), data: serde_json::to_value(&c).unwrap_or(Value::Null) }], "战斗中更换装备")?;
    Ok(())
}

/// 主持人强制结束战斗（例如对方投降、剧情打断）。不发放经验。
pub fn abort(world: &mut World, campaign_id: &str, reason: &str) -> Result<CombatReport> {
    let mut c = active(world.conn(), campaign_id).ok_or_else(|| CoreError::Invalid("现在没有在战斗".into()))?;
    let mut lines = vec![];
    c.status = "ended".into();
    c.push(&mut lines, format!("战斗中止：{reason}"));
    save(world, campaign_id, &mut c, &mut lines)?;
    Ok(CombatReport { lines, combat: c, dice: vec![] })
}

/// 保存战斗状态；战斗结束时把生命值、伤势写回角色卡，并发放经验与战利品。
/// 为什么无法逃跑（None = 可以尝试）：场地封锁、首领还在、四面被围。
fn escape_blocked(c: &Combat, pi: usize) -> Option<String> {
    if !c.no_escape.is_empty() {
        return Some(c.no_escape.clone());
    }
    if let Some(b) = c.units.iter().find(|u| u.boss && u.active() && !u.friendly()) {
        return Some(format!("「{}」不会让你轻易离开，首领战无法逃跑", b.name));
    }
    let p = &c.units[pi];
    let blocked = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().all(|(dx, dy)| {
        let (x, y) = (p.x + dx, p.y + dy);
        x < 0 || y < 0 || x >= c.width || y >= c.height || c.obstacles.contains(&(x, y)) || c.units.iter().any(|u| u.active() && !u.friendly() && u.x == x && u.y == y)
    });
    blocked.then(|| "四面都被敌人和障碍挡住，被包围了".to_string())
}

fn save(world: &mut World, campaign_id: &str, c: &mut Combat, lines: &mut Vec<String>) -> Result<()> {
    let ended = c.status != "active";
    let mut eff = vec![];
    let mut xp_total = 0;
    let mut loot: Vec<(String, i64, String)> = vec![];
    if ended {
        let r = rpg::rules(world.conn(), campaign_id);
        for u in &c.units {
            if let Some(actor) = &u.actor {
                let mut s = rpg::sheet(world.conn(), campaign_id, actor)?;
                s.hp = u.hp.max(0);
                s.wounds.extend(u.new_wounds.iter().cloned());
                s.statuses.retain(|x| x.rounds.is_none());
                eff.push(rpg::sheet_effect(&s));
                if u.out == "dead" {
                    eff.push(Effect::SetEntityState { entity: actor.clone(), field: "life_status".into(), value: json!("dead") });
                }
                if actor == PARTY && u.hp <= 0 && r.party_death == "dead" {
                    eff.push(Effect::SetVar { key: "party.dead".into(), value: json!(true) });
                }
            }
            if c.status == "victory" && !u.friendly() && (u.out == "down" || u.out == "dead") {
                xp_total += u.xp;
                for l in &u.loot {
                    loot.push((l.item.clone(), l.qty.max(1), u.name.clone()));
                }
            }
        }
    }
    eff.push(Effect::UpsertRecord { kind: "combat".into(), id: c.id.clone(), data: serde_json::to_value(&*c).unwrap_or(Value::Null) });
    eff.push(Effect::SetVar { key: ACTIVE_VAR.into(), value: if ended { Value::Null } else { json!(c.id) } });
    campaign::apply_effects(world, campaign_id, &eff, &format!("战斗：第 {} 轮", c.round))?;
    if ended && c.status == "victory" {
        let mut rewards = vec![];
        if xp_total > 0 {
            // 经验：参战的每名玩家都拿到（联机时人人有份；单人就是玩家自己）。
            let players: Vec<String> = c.units.iter().filter_map(|u| u.actor.clone()).filter(|a| rpg::is_player_actor(a)).collect();
            for p in if players.is_empty() { vec![rpg::actor()] } else { players } {
                rewards.extend(rpg::award_xp(world, campaign_id, &p, xp_total, "战斗胜利")?);
            }
        }
        for (item, qty, from) in loot {
            if let Some(it) = rpg::item_info(world.conn(), &item)? {
                rpg::give_item(world, campaign_id, &rpg::actor(), &item, qty, &format!("战利品：{from}"))?;
                rewards.push(format!("从{from}身上得到「{}」×{qty}", it.title));
            }
        }
        if !rewards.is_empty() {
            for t in &rewards {
                lines.push(t.clone());
            }
            c.rewards = rewards;
            campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "combat".into(), id: c.id.clone(), data: serde_json::to_value(&*c).unwrap_or(Value::Null) }], "战斗结算")?;
        }
    }
    Ok(())
}

/// 给 GM 看的战斗摘要。
pub fn summary(c: &Combat) -> String {
    let mut s = format!("第 {} 轮；{}\n", c.round, match c.current() { Some(u) if c.status == "active" => format!("轮到 {}（剩余行动点 {}）", u.name, u.ap), _ => format!("战斗已结束：{}", c.status) });
    for u in &c.units {
        let st: Vec<String> = u.statuses.iter().map(|x| x.name.clone()).collect();
        s.push_str(&format!(
            "- {}（{}）生命 {}/{}，防御 {}，{}（射程 {}），位置 {},{}{}{}\n",
            u.name,
            match u.side.as_str() { "party" => "玩家", "ally" => "同伴", _ => "敌人" },
            u.hp.max(0),
            u.max_hp,
            u.ac_now(),
            u.weapon,
            u.range,
            u.x,
            u.y,
            if st.is_empty() { String::new() } else { format!("，状态：{}", st.join("、")) },
            match u.out.as_str() { "down" => "，已倒下", "fled" => "，已逃走", "dead" => "，已死亡", _ => "" }
        ));
    }
    s
}
