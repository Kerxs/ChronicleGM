//! 世界模拟：环境（昼夜、季节、天气）、地图与旅行、探索状态与标记、势力按天推演、事件触发器、
//! 环境对 NPC 日程的影响、时间线回溯，以及长时间跳跃的性能。

mod common;

use std::time::Instant;

use chronicle_core::campaign;
use chronicle_core::schedule;
use chronicle_core::timeline;
use chronicle_core::worldsim;
use chronicle_core::*;
use common::*;
use serde_json::{Value, json};

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

fn rel(w: &mut World, a: &str, b: &str, t: &str, label: &str, directed: bool) {
    let mut r = NewRelation::new(a, b, t);
    r.label = label.into();
    r.directed = directed;
    w.write(&user(), "r", |x| x.create_relation(r)).unwrap();
}

struct Map {
    e: Env,
    a: String,
    forge: String,
    b: String,
    c: String,
    d: String,
    far: String,
    camp: String,
}

/// 灰石村(A) ⊃ 铁匠铺；A —2小时→ 黑木镇(B)；B —半天→ 白塔城(C)（接壤）；C → 遗迹(D) 单向 90 分钟；远方(F) 不通路。
fn map_fx() -> Map {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::City, "灰石村")).id;
    let forge = mk(&mut e.world, NewNode::new(NodeKind::Site, "铁匠铺").under(&a)).id;
    let b = mk(&mut e.world, NewNode::new(NodeKind::City, "黑木镇")).id;
    let c = mk(&mut e.world, NewNode::new(NodeKind::City, "白塔城")).id;
    let d = mk(&mut e.world, NewNode::new(NodeKind::Site, "古代遗迹")).id;
    let far = mk(&mut e.world, NewNode::new(NodeKind::City, "远方之城")).id;
    rel(&mut e.world, &a, &b, "route_to", "2小时", false);
    rel(&mut e.world, &b, &c, "borders", "半天", false);
    rel(&mut e.world, &c, &d, "route_to", "90分钟", true);
    let camp = campaign::create_campaign(&mut e.world, None, "旅行", Some(7)).unwrap().id;
    eff(&mut e.world, &camp, json!([{"type": "set_var", "key": "party.location", "value": a}]));
    Map { e, a, forge, b, c, d, far, camp }
}

#[test]
fn environment_is_reproducible_and_written_to_vars() {
    let mut e = env();
    let c1 = campaign::create_campaign(&mut e.world, None, "一", Some(11)).unwrap().id;
    let c2 = campaign::create_campaign(&mut e.world, None, "二", Some(11)).unwrap().id;
    let w = &mut e.world;
    let e0 = worldsim::environment(w.conn(), &c1).unwrap();
    assert_eq!((e0.day, e0.season.as_str(), e0.daypart.as_str()), (1, "春", "白天"));
    schedule::advance(w, &c1, 13 * 60, "x").unwrap();
    let e1 = worldsim::environment(w.conn(), &c1).unwrap();
    assert_eq!(e1.daypart, "夜晚", "21:00 是夜晚");
    assert!(e1.travel_factor >= 1.3);
    // 同样的种子、同样的日子，天气相同（可复现）。
    let ws1: Vec<String> = (1..=60).map(|m| worldsim::environment_at(w.conn(), &c1, m * 1440).unwrap().weather).collect();
    let ws2: Vec<String> = (1..=60).map(|m| worldsim::environment_at(w.conn(), &c2, m * 1440).unwrap().weather).collect();
    assert_eq!(ws1, ws2);
    assert!(ws1.iter().collect::<std::collections::HashSet<_>>().len() >= 3, "天气有变化：{ws1:?}");
    // 环境写入变量，条件可以引用。
    let view = campaign::CampaignView { conn: w.conn(), campaign_id: &c1 };
    use chronicle_rules::StateView;
    assert_eq!(view.var("env.daypart"), Some(json!("夜晚")));
    assert_eq!(view.var("env.season"), Some(json!("春")));
    // 季节长度可配置；跳到第 31 天进入夏季。
    eff(w, &c1, json!([{"type": "set_var", "key": "cal.days_per_season", "value": 10}]));
    assert_eq!(worldsim::environment_at(w.conn(), &c1, 10 * 1440 + 600).unwrap().season, "夏");
    // 剧情指定天气。
    let day = worldsim::environment(w.conn(), &c1).unwrap().day;
    eff(w, &c1, json!([{"type": "set_var", "key": "env.weather_override", "value": {"day": day, "weather": "暴雨"}}]));
    let e2 = worldsim::environment(w.conn(), &c1).unwrap();
    assert!(e2.bad_weather && e2.weather == "暴雨");
}

#[test]
fn travel_uses_shortest_route_time_and_weather() {
    let mut f = map_fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let p = worldsim::plan_travel(w.conn(), &c, &f.c).unwrap();
    assert_eq!(p.base_minutes, 120 + 360, "道路 2 小时 + 边境半天");
    assert_eq!(p.legs.iter().map(|l| l.to.clone()).collect::<Vec<_>>(), vec![f.b.clone(), f.c.clone()]);
    assert_eq!(p.legs[1].via, "边境");
    // 进入城内的地点：上下级 15 分钟。
    assert_eq!(worldsim::plan_travel(w.conn(), &c, &f.forge).unwrap().base_minutes, 15);
    // 不通路的地点给出明确错误；非地点不能作为目的地。
    assert!(worldsim::plan_travel(w.conn(), &c, &f.far).unwrap_err().to_string().contains("没有通往"));
    // 恶劣天气让旅行变慢。
    let day = worldsim::environment(w.conn(), &c).unwrap().day;
    eff(w, &c, json!([{"type": "set_var", "key": "env.weather_override", "value": {"day": day, "weather": "暴雪"}}]));
    let slow = worldsim::plan_travel(w.conn(), &c, &f.c).unwrap();
    assert_eq!(slow.factor, 2.0);
    assert_eq!(slow.minutes, 960);
    // 出发：时间按路程推进。
    let before = schedule::clock(w.conn(), &c);
    let (plan, _) = worldsim::travel(w, &c, &f.c, "测试").unwrap();
    assert_eq!(schedule::clock(w.conn(), &c), before + plan.minutes);
    assert_eq!(worldsim::party_location(w.conn(), &c).unwrap().as_deref(), Some(f.c.as_str()));
    // 单向道路：C → D 可以，D → C 不行。
    worldsim::travel(w, &c, &f.d, "测试").unwrap();
    assert!(worldsim::plan_travel(w.conn(), &c, &f.c).is_err(), "单向道路不能原路返回");
    timeline::end_campaign(w, &c, "").unwrap();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok);
}

/// 实测问题回归：同一区域的两座城市不能经过区域节点"抄近路"；所在地点及其上级不算"听说"。
#[test]
fn containment_is_not_a_shortcut_and_known_places_are_not_heard_again() {
    let mut e = env();
    let w = &mut e.world;
    let region = mk(w, NewNode::new(NodeKind::Region, "北境")).id;
    let village = mk(w, NewNode::new(NodeKind::City, "灰石村").under(&region)).id;
    let tavern = mk(w, NewNode::new(NodeKind::Site, "酒馆").under(&village)).id;
    let town = mk(w, NewNode::new(NodeKind::City, "黑木镇").under(&region)).id;
    rel(w, &village, &town, "route_to", "3小时", false);
    let c = campaign::create_campaign(w, None, "c", Some(1)).unwrap().id;
    eff(w, &c, json!([{"type": "set_var", "key": "party.location", "value": tavern}]));
    let p = worldsim::plan_travel(w.conn(), &c, &town).unwrap();
    assert_eq!(p.base_minutes, 15 + 180, "从酒馆出村 15 分钟，再走 3 小时");
    assert!(worldsim::plan_travel(w.conn(), &c, &region).unwrap_err().to_string().contains("具体的城市或地点"));
    assert!(worldsim::hear_of_places(w, &c, &[village.clone(), region.clone(), town.clone(), town.clone()]).unwrap() == vec!["黑木镇".to_string()]);
    // 超过 7 天的路程完整推进。
    let distant = mk(w, NewNode::new(NodeKind::City, "极北冰原")).id;
    rel(w, &village, &distant, "route_to", "10天", false);
    let before = schedule::clock(w.conn(), &c);
    let (p, _) = worldsim::travel(w, &c, &distant, "x").unwrap();
    assert!(p.minutes >= 10 * 1440);
    assert_eq!(schedule::clock(w.conn(), &c), before + p.minutes);
    worldsim::travel(w, &c, &village, "x").unwrap();
    // 离开区域之后，去过的区域仍然是已探索。
    let far = mk(w, NewNode::new(NodeKind::City, "白塔城")).id;
    rel(w, &town, &far, "borders", "1天", false);
    worldsim::travel(w, &c, &far, "x").unwrap();
    let m = worldsim::map(w.conn(), &c, false).unwrap();
    let st = |id: &str| m.places.iter().find(|p| p.id == id).map(|p| p.state.clone());
    assert_eq!((st(&region).as_deref(), st(&tavern).as_deref(), st(&town).as_deref()), (Some("explored"), Some("explored"), Some("explored")));
}

#[test]
fn exploration_states_and_markers() {
    let mut f = map_fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let state = |w: &World, id: &str, gm: bool| worldsim::map(w.conn(), &c, gm).unwrap().places.into_iter().find(|p| p.id == id).map(|p| p.state);
    // 开始时：所在地点已探索，其他地点未知（玩家看不到）。
    assert_eq!(state(w, &f.a, false).as_deref(), Some("explored"));
    assert_eq!(state(w, &f.b, false), None);
    assert_eq!(state(w, &f.b, true).as_deref(), Some("unknown"), "主持人视角能看到未知地点");
    // 听说了黑木镇：未探索。
    let names = worldsim::hear_of_places(w, &c, std::slice::from_ref(&f.b)).unwrap();
    assert_eq!(names, vec!["黑木镇".to_string()]);
    assert_eq!(state(w, &f.b, false).as_deref(), Some("heard"));
    // 旅行到黑木镇：已探索；它的邻居白塔城变成"听说过"。
    worldsim::travel(w, &c, &f.b, "测试").unwrap();
    assert_eq!(state(w, &f.b, false).as_deref(), Some("explored"));
    assert_eq!(state(w, &f.c, false).as_deref(), Some("heard"));
    assert_eq!(state(w, &f.d, false), None);
    let m = worldsim::map(w.conn(), &c, false).unwrap();
    assert!(m.places.iter().find(|p| p.id == f.b).unwrap().here);
    // 标记。
    assert!(worldsim::add_marker(w, &c, &f.c, " ", "").is_err());
    let id = worldsim::add_marker(w, &c, &f.c, "据说有宝藏", "酒馆老板说的").unwrap();
    let m = worldsim::map(w.conn(), &c, false).unwrap();
    assert_eq!(m.places.iter().find(|p| p.id == f.c).unwrap().markers[0].label, "据说有宝藏");
    worldsim::remove_marker(w, &c, &id).unwrap();
    assert!(worldsim::map(w.conn(), &c, false).unwrap().places.iter().all(|p| p.markers.is_empty()));
    // 附近地点按路程排序。
    let near = worldsim::reachable(w.conn(), &c, 10).unwrap();
    assert_eq!(near[0].id, f.a);
    assert_eq!(near.iter().find(|r| r.id == f.c).unwrap().base_minutes, 360);
}

fn faction(w: &mut World, name: &str, goals: &[&str], power: Value) -> String {
    mk(w, NewNode::new(NodeKind::Faction, name).payload(json!({ "goals": goals, "power": power }))).id
}

#[test]
fn factions_act_daily_war_battle_peace_alliance_coup() {
    let mut e = env();
    let w = &mut e.world;
    let north = faction(w, "北境王国", &["扩张领土"], json!({"wealth": 60, "military": 85, "influence": 50, "stability": 70}));
    let south = faction(w, "南方公国", &["保住王位"], json!({"wealth": 60, "military": 75, "influence": 40, "stability": 70}));
    let east = faction(w, "东海商盟", &["垄断贸易"], json!({"wealth": 70, "military": 40, "influence": 60, "stability": 70}));
    let rebels = faction(w, "动荡之城", &[], json!({"wealth": 5, "military": 20, "influence": 30, "stability": 10}));
    let guild = faction(w, "行商公会", &[], json!({"wealth": 50, "military": 10, "influence": 30, "stability": 60}));
    rel(w, &north, &south, "war", "", false);
    rel(w, &east, &south, "war", "", false);
    rel(w, &guild, &east, "trade", "", false);
    let c = campaign::create_campaign(w, None, "势力", Some(3)).unwrap().id;
    let fs = worldsim::factions(w.conn(), &c).unwrap();
    assert_eq!(fs.iter().find(|f| f.id == north).unwrap().at_war_with, vec![south.clone()]);
    assert_eq!(fs.iter().find(|f| f.id == guild).unwrap().trade_with, vec![east.clone()]);
    for _ in 0..9 {
        schedule::advance(w, &c, 7 * 1440, "过了一周").unwrap();
    }
    let evs = worldsim::world_events(w.conn(), &c, 500).unwrap();
    let kinds: std::collections::HashSet<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains("battle"), "{kinds:?}");
    assert!(kinds.contains("peace"), "南方公国军力崩溃后停战：{kinds:?}");
    assert!(kinds.contains("coup"), "动荡之城发生政变：{kinds:?}");
    assert!(kinds.contains("alliance"), "北境与东海因共同的敌人结盟：{kinds:?}");
    let fs = worldsim::factions(w.conn(), &c).unwrap();
    let get = |id: &str| fs.iter().find(|f| f.id == id).unwrap().clone();
    assert!(get(&south).power.military < 75, "两线作战的一方军力下降");
    assert!(get(&south).at_war_with.len() < 2, "至少一场战争已经停战");
    assert!(get(&guild).power.wealth > 50, "贸易带来财富");
    assert!(get(&rebels).power.stability >= 20 || evs.iter().any(|e| e.kind == "coup"));
    assert!(fs.iter().all(|f| !f.last_action.is_empty()));
    // 每个事件都有效果记录（时间线可以重放）。
    timeline::end_campaign(w, &c, "").unwrap();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok);
    // 确定性：同样的种子得到同样的世界历史。
    let c2 = campaign::create_campaign(w, None, "势力2", Some(3)).unwrap().id;
    for _ in 0..9 {
        schedule::advance(w, &c2, 7 * 1440, "过了一周").unwrap();
    }
    let texts = |id: &str| worldsim::world_events(w.conn(), id, 500).unwrap().into_iter().map(|e| e.text).collect::<Vec<_>>();
    assert_eq!(texts(&c), texts(&c2));
}

#[test]
fn script_triggers_fire_once_when_conditions_hold() {
    let mut e = env();
    let u = user();
    let s = e.world.write(&u, "s", |w| w.create_script(NewScript::new("戒严"))).unwrap().0;
    let mut it = NewScriptItem::new(&s.id, ScriptItemKind::Trigger, "城门戒严");
    it.payload = json!({
        "when": {"type": "all", "of": [{"type": "flag", "name": "alarm"}, {"type": "var", "key": "env.daypart", "op": "eq", "value": "夜晚"}]},
        "effects": [{"type": "set_var", "key": "guards", "value": 5}],
        "text": "夜里城门戒严，守卫增加到五人"
    });
    e.world.write(&u, "i", |w| w.create_script_item(it)).unwrap();
    let c = campaign::create_campaign(&mut e.world, Some(&s.id), "c", Some(1)).unwrap().id;
    let w = &mut e.world;
    let fired = |w: &World| worldsim::world_events(w.conn(), &c, 50).unwrap().into_iter().filter(|e| e.kind == "trigger").count();
    schedule::advance(w, &c, 60, "x").unwrap();
    assert_eq!(fired(w), 0, "条件不满足");
    eff(w, &c, json!([{"type": "set_flag", "name": "alarm"}]));
    schedule::advance(w, &c, 60, "x").unwrap();
    assert_eq!(fired(w), 0, "还是白天");
    let notes = schedule::advance(w, &c, 12 * 60, "等到夜里").unwrap();
    assert_eq!(fired(w), 1);
    assert!(notes.iter().any(|n| n.contains("城门戒严")), "{notes:?}");
    use chronicle_rules::StateView;
    assert_eq!(campaign::CampaignView { conn: w.conn(), campaign_id: &c }.var("guards"), Some(json!(5)));
    schedule::advance(w, &c, 10, "x").unwrap();
    assert_eq!(fired(w), 1, "只触发一次");
}

#[test]
fn bad_weather_keeps_outdoor_npcs_home() {
    let mut e = env();
    let u = user();
    let field = mk(&mut e.world, NewNode::new(NodeKind::Site, "麦田"));
    let home = mk(&mut e.world, NewNode::new(NodeKind::Site, "农舍"));
    let farmer = mk(
        &mut e.world,
        NewNode::new(NodeKind::Character, "农夫").payload(json!({
            "home": home.id,
            "schedule": [{"start": "06:00", "end": "18:00", "activity": "下地干活", "location": field.id, "outdoor": true}]
        })),
    );
    let s = e.world.write(&u, "s", |w| w.create_script(NewScript::new("雨天"))).unwrap().0;
    e.world.write(&u, "c", |w| w.add_cast(NewCastMember { script_id: s.id.clone(), node_id: farmer.id.clone(), role: String::new(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })).unwrap();
    let c = campaign::create_campaign(&mut e.world, Some(&s.id), "c", Some(1)).unwrap().id;
    let w = &mut e.world;
    let fnode = nodes::get_node(w.conn(), &farmer.id).unwrap().unwrap();
    let st = schedule::npc_status(w.conn(), &c, &fnode).unwrap();
    // 第 1 天的天气由种子决定；先确认晴天时在麦田，再指定暴雨。
    eff(w, &c, json!([{"type": "set_var", "key": "env.weather_override", "value": {"day": 1, "weather": "晴"}}]));
    let st_sunny = schedule::npc_status(w.conn(), &c, &fnode).unwrap();
    assert_eq!(st_sunny.location_id.as_deref(), Some(field.id.as_str()));
    eff(w, &c, json!([{"type": "set_var", "key": "env.weather_override", "value": {"day": 1, "weather": "暴雨"}}]));
    let st_rain = schedule::npc_status(w.conn(), &c, &fnode).unwrap();
    assert_eq!(st_rain.location_id.as_deref(), Some(home.id.as_str()));
    assert!(st_rain.activity.contains("暴雨") && st_rain.activity.contains("下地干活"), "{}", st_rain.activity);
    let _ = st;
}

#[test]
fn world_simulation_follows_timelines() {
    let mut f = map_fx();
    let w = &mut f.e.world;
    let k1 = faction(w, "甲国", &["征服"], json!({"military": 80}));
    let k2 = faction(w, "乙国", &[], json!({"military": 40}));
    rel(w, &k1, &k2, "war", "", false);
    let c = f.camp.clone();
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    let cp = timeline::checkpoints(w.conn(), &c).unwrap().last().unwrap().id;
    worldsim::travel(w, &c, &f.b, "测试").unwrap();
    worldsim::add_marker(w, &c, &f.b, "旅店", "").unwrap();
    for _ in 0..3 {
        schedule::advance(w, &c, 7 * 1440, "x").unwrap();
    }
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    timeline::end_campaign(w, &c, "").unwrap();
    assert!(!worldsim::world_events(w.conn(), &c, 10).unwrap().is_empty());
    let b = timeline::fork(w, &c, cp, "", false).unwrap();
    assert_eq!(worldsim::party_location(w.conn(), &b.id).unwrap().as_deref(), Some(f.a.as_str()), "回到出发前的位置");
    assert!(worldsim::world_events(w.conn(), &b.id, 10).unwrap().is_empty());
    assert!(worldsim::map(w.conn(), &b.id, false).unwrap().places.iter().all(|p| p.markers.is_empty()));
    assert_eq!(worldsim::factions(w.conn(), &b.id).unwrap().iter().find(|x| x.id == k2).unwrap().power.military, 40);
    let cmp = timeline::compare(w.conn(), &[c.clone(), b.id.clone()]).unwrap();
    assert!(cmp.rows.iter().any(|r| r.category == "世界状态" && r.key.contains("势力") && r.differs), "两条时间线的势力不同");
    assert!(cmp.rows.iter().any(|r| r.category == "世界状态" && r.key == "世界事件数" && r.differs));
    for id in [&c, &b.id] {
        assert!(timeline::verify(w.conn(), id).unwrap().ok);
    }
}

/// 性能：20 个势力、100 天的批量推进（不调用模型）。
#[test]
fn long_time_jump_with_many_factions_is_fast() {
    let mut e = env();
    let w = &mut e.world;
    let ids: Vec<String> = (0..20)
        .map(|i| faction(w, &format!("势力{i}"), if i % 3 == 0 { &["扩张"] } else { &[] }, json!({"military": 30 + i * 3, "wealth": 20 + i * 2})))
        .collect();
    for i in (0..20).step_by(4) {
        rel(w, &ids[i], &ids[i + 1], "war", "", false);
        rel(w, &ids[i + 2], &ids[i + 3], "trade", "", false);
    }
    let c = campaign::create_campaign(w, None, "长跳", Some(9)).unwrap().id;
    let t = Instant::now();
    let mut days = 0;
    while days < 100 {
        schedule::advance(w, &c, 7 * 1440, "跳过一周").unwrap();
        days += 7;
    }
    let el = t.elapsed();
    let n = worldsim::world_events(w.conn(), &c, 10_000).unwrap().len();
    println!("20 个势力推进 {days} 天：{:?}，世界事件 {n} 条", el);
    assert!(el.as_secs_f64() < 20.0, "{el:?}");
    timeline::end_campaign(w, &c, "").unwrap();
    let t = Instant::now();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok);
    println!("重放校验：{:?}", t.elapsed());
}
