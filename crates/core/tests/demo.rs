//! 示例世界：内容完整（剧本检查没有必须处理的问题），可以直接开战役、选角色。

mod common;

use chronicle_core::*;
use common::*;

#[test]
fn demo_world_is_complete_and_playable() {
    let mut e = env();
    let sid = demo::populate(&mut e.world).unwrap();
    let w = &mut e.world;
    let errors: Vec<_> = scripts::check_script(w.conn(), &sid).unwrap().into_iter().filter(|f| f.severity == "error").map(|f| f.message).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let pcs = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::PlayerCharacter).unwrap();
    assert_eq!(pcs.len(), 2);
    let gp = profile::for_script(&scripts::require_script(w.conn(), &sid).unwrap());
    assert!(gp.weights.investigation <= 1, "示例剧本不是探案");
    assert_eq!(gp.source, "user");
    let c = campaign::create_campaign(w, Some(&sid), "教程", Some(3)).unwrap();
    let pc = pc::from_script_item(&pcs[0]);
    pc::set(w, &c.id, &pc).unwrap();
    assert_eq!(rpg::sheet(w.conn(), &c.id, rpg::PARTY).unwrap().name, pcs[0].title);
    assert!(campaign::current_node(w, &c.id).unwrap().is_some(), "有开始节点");
}
