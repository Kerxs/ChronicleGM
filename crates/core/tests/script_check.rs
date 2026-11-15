//! 剧本检查（程序）：缺开场 / 章节为错误，缺主线 / 结局 / 可选角色为警告，可选角色缺身世、线索过多、地点不在世界树里为提示。

mod common;

use chronicle_core::*;
use common::*;
use serde_json::json;

fn item(w: &mut World, sid: &str, kind: ScriptItemKind, title: &str, payload: serde_json::Value) -> String {
    let mut it = NewScriptItem::new(sid, kind, title);
    it.payload = payload;
    w.write(&user(), "i", |x| x.create_script_item(it)).unwrap().0.id
}

#[test]
fn findings_cover_completeness_and_profile_fit() {
    let mut e = env();
    let mut ns = NewScript::new("护送商队");
    ns.genre = "奇幻冒险".into();
    let s = e.world.write(&user(), "s", |x| x.create_script(ns)).unwrap().0;
    let f = scripts::check_script(e.world.conn(), &s.id).unwrap();
    let msgs = |f: &Vec<scripts::ScriptFinding>, sev: &str| f.iter().filter(|x| x.severity == sev).map(|x| x.message.clone()).collect::<Vec<_>>();
    assert!(msgs(&f, "error").contains(&"没有开场".to_string()));
    assert!(msgs(&f, "error").contains(&"没有章节".to_string()));
    assert!(msgs(&f, "warning").iter().any(|m| m.contains("可选角色")));
    assert!(msgs(&f, "info").iter().any(|m| m.contains("按题材推断")));

    item(&mut e.world, &s.id, ScriptItemKind::Opening, "开场", json!({}));
    item(&mut e.world, &s.id, ScriptItemKind::Chapter, "第一章", json!({}));
    item(&mut e.world, &s.id, ScriptItemKind::Quest, "护送", json!({"quest_type": "main"}));
    item(&mut e.world, &s.id, ScriptItemKind::Ending, "抵达", json!({}));
    let pc = item(&mut e.world, &s.id, ScriptItemKind::PlayerCharacter, "护卫", json!({"identity": "佣兵"}));
    item(&mut e.world, &s.id, ScriptItemKind::Scene, "山口", json!({"location_name": "鹰嘴关"}));
    for i in 0..9 {
        item(&mut e.world, &s.id, ScriptItemKind::Clue, &format!("线索{i}"), json!({}));
    }
    let f = scripts::check_script(e.world.conn(), &s.id).unwrap();
    assert!(msgs(&f, "error").is_empty(), "{:?}", msgs(&f, "error"));
    let no_story = f.iter().find(|x| x.message.contains("没有身世")).expect("缺身世的提示");
    assert_eq!(no_story.item_id.as_deref(), Some(pc.as_str()));
    assert!(msgs(&f, "info").iter().any(|m| m.contains("鹰嘴关")), "地点不在世界树里");
    assert!(msgs(&f, "info").iter().any(|m| m.contains("线索有 9 条")), "奇幻冒险（调查比重 1）线索太多：{:?}", msgs(&f, "info"));
}
