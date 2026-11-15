//! 第 23 项：世界树节点的资料字段随题材变化（生成提示词里带上题材字段）。

use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gen::world_tasks::{SeedInput, build_world_seed};

#[test]
fn wuxia_world_seed_asks_for_sect_and_martial_arts() {
    let dir = tempfile::tempdir().unwrap();
    let w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "x").unwrap();
    let (user, _) = build_world_seed(&w, &Templates::builtin(), 4000, SeedInput { idea: "江湖门派之争", genre: "武侠", era: "", style: "", rules: "" }).unwrap();
    assert!(user.contains("sect：所属门派") && user.contains("martial_arts"), "武侠题材要求写门派和武功");
    let (plain, _) = build_world_seed(&w, &Templates::builtin(), 4000, SeedInput { idea: "一个普通的小镇", genre: "日常", era: "", style: "", rules: "" }).unwrap();
    assert!(!plain.contains("sect："), "其他题材不加武侠字段");
    let sci = profile::infer("科幻", "", "");
    assert!(sci.node_fields_hint("character").contains("implants"));
}
