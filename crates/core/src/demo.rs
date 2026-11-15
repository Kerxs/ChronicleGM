//! 示例世界「雾湾 · 灯塔守夜」：手写的原创小剧本，给游戏教程用。
//! 包含世界树（地区、小镇、地点、人物、势力、物品）、剧本（开场、主线任务、两个带身世的可选角色、结局）、角色与剧情图。
//! 不需要 AI 生成，导入后就能开新战役（GM 回合仍然需要模型）。

use serde_json::json;

use crate::error::Result;
use crate::model::*;
use crate::world::World;

pub const DEMO_WORLD_NAME: &str = "示例世界：雾湾";
pub const DEMO_SCRIPT_TITLE: &str = "灯塔守夜";

fn node(w: &mut World, kind: NodeKind, title: &str, parent: Option<&str>, summary: &str, player_text: &str, payload: serde_json::Value) -> Result<String> {
    let mut n = NewNode::new(kind, title).payload(payload);
    n.parent_id = parent.map(str::to_string);
    n.summary = summary.into();
    n.player_text = player_text.into();
    Ok(w.write(&Author::User, "示例世界", |x| x.create_node(n))?.0.id)
}

#[allow(clippy::too_many_arguments)]
fn item(w: &mut World, sid: &str, kind: ScriptItemKind, title: &str, parent: Option<&str>, player_text: &str, gm_text: &str, payload: serde_json::Value) -> Result<String> {
    let mut it = NewScriptItem::new(sid, kind, title);
    it.parent_id = parent.map(str::to_string);
    it.player_text = player_text.into();
    it.gm_text = gm_text.into();
    it.payload = payload;
    Ok(w.write(&Author::User, "示例剧本", |x| x.create_script_item(it))?.0.id)
}

/// 在（空的）世界里写入示例内容，返回剧本编号。
pub fn populate(w: &mut World) -> Result<String> {
    // ---------- 世界树 ----------
    let bay = node(w, NodeKind::Region, "雾湾", None, "常年起雾的海湾，渔村沿着礁石散开。", "雾气从海上漫进来，船只靠灯塔认路。", json!({}))?;
    let town = node(w, NodeKind::City, "雾灯镇", Some(&bay), "雾湾最大的渔镇，几百户人家靠打渔为生。", "石头房子挤在码头边，屋檐下挂着防雾的铜铃。", json!({}))?;
    let tower = node(w, NodeKind::Site, "老灯塔", Some(&town), "镇子北边岬角上的灯塔，三十年来从没熄过。", "白色的塔身被海风磨得发灰，顶上本该亮着的灯今晚是暗的。", json!({}))?;
    let cave = node(w, NodeKind::Site, "潮洞", Some(&bay), "灯塔下方的海蚀洞，退潮时才能走进去。", "洞口挂着湿漉漉的海藻，里面传来滴水声和奇怪的窸窣声。", json!({}))?;
    let keeper = node(
        w,
        NodeKind::Character,
        "莫伦",
        Some(&town),
        "老灯塔的守塔人，七十岁，固执又心软。",
        "满脸皱纹的老人，嗓门很大，手上全是老茧。",
        json!({"identity": "守塔人", "personality": "固执、嘴硬心软", "secrets": ["灯芯石是他年轻时从潮洞深处取来的，洞里的海妖一直想把它拿回去"], "knowledge": ["备用的灯芯石藏在潮洞的石台上", "退潮只有一个时辰"]}),
    )?;
    let mayor = node(
        w,
        NodeKind::Character,
        "苏桦",
        Some(&town),
        "雾灯镇的镇长，精明干练，担心出海的船队。",
        "穿着整洁蓝外套的中年女人，说话很快。",
        json!({"identity": "镇长", "personality": "精明、务实、护着镇民", "secrets": ["她的儿子就在今晚回港的船队里"]}),
    )?;
    let lan = node(
        w,
        NodeKind::Character,
        "阿澜",
        Some(&town),
        "渔家女，水性最好，熟悉潮洞的路。",
        "晒得黝黑的姑娘，腰上别着一把剖鱼刀。",
        json!({"identity": "渔家女", "personality": "爽快、胆大", "knowledge": ["潮洞里住着几只小海妖，怕火光"]}),
    )?;
    node(w, NodeKind::Faction, "渔民互助会", Some(&town), "雾灯镇渔民的互助组织，船队就是他们的。", "码头边那栋挂着网的大屋就是互助会。", json!({}))?;
    let stone = node(w, NodeKind::Item, "灯芯石", None, "会发光的蓝色石头，灯塔的灯靠它点亮。", "拳头大小，摸上去是温的，在暗处发出蓝光。", json!({"value": 500, "rarity": "rare"}))?;
    let _ = stone;

    // ---------- 剧本 ----------
    let mut ns = NewScript::new(DEMO_SCRIPT_TITLE);
    ns.genre = "海边冒险".into();
    ns.tone = "温暖、带一点紧张".into();
    ns.premise = "风暴前夜，老灯塔的灯熄了，出海的船队要在天亮前回港。".into();
    ns.background = "雾灯镇的老灯塔三十年来从没熄过。今晚起了大雾，风暴正在逼近，船队还在海上。".into();
    ns.core_setting = "灯塔的灯靠灯芯石发光；灯芯石来自潮洞深处，洞里住着怕火光的小海妖。".into();
    ns.player_count = 1;
    ns.duration_hours = 1.0;
    let mut gp = crate::profile::infer(&ns.genre, &ns.tone, &ns.premise);
    gp.weights.combat = 1;
    gp.weights.exploration = 2;
    gp.weights.social = 2;
    gp.weights.investigation = 1;
    gp.weights.survival = 1;
    gp.weights.management = 0;
    gp.source = "user".into();
    ns.game_profile = serde_json::to_value(&gp)?;
    let s = w.write(&Author::User, "示例剧本", |x| x.create_script(ns))?.0;
    let sid = s.id.clone();

    item(
        w,
        &sid,
        ScriptItemKind::Opening,
        "开场：熄灭的灯塔",
        None,
        "傍晚，雾从海上漫进雾灯镇。码头上的人都抬头望向北边的岬角——老灯塔的灯，熄了。镇长苏桦站在码头的木箱上大喊：船队天亮前就要回港，没有灯，他们会撞上礁石。",
        "莫伦的灯芯石碎了。备用的灯芯石在潮洞里，退潮只有一个时辰。",
        json!({}),
    )?;
    let ch = item(w, &sid, ScriptItemKind::Chapter, "第一章：风暴前夜", None, "在风暴到来之前重新点亮老灯塔。", "", json!({}))?;
    item(w, &sid, ScriptItemKind::Scene, "码头", Some(&ch), "码头上挤满了焦急的镇民。", "苏桦会请求玩家帮忙；阿澜愿意带路。", json!({"location_name": "雾灯镇"}))?;
    item(w, &sid, ScriptItemKind::Scene, "老灯塔", Some(&ch), "灯室里一片漆黑，碎掉的灯芯石散在地上。", "莫伦嘴硬不肯求人，但说服他后会说出备用灯芯石在潮洞。", json!({"location_name": "老灯塔"}))?;
    item(w, &sid, ScriptItemKind::Scene, "潮洞", Some(&ch), "退潮后的海蚀洞，石台上隐约有蓝光。", "几只小海妖守着石台，它们怕火光；也可以用食物把它们引开，不一定要打。", json!({"location_name": "潮洞"}))?;
    let quest = item(
        w,
        &sid,
        ScriptItemKind::Quest,
        "点亮老灯塔",
        None,
        "天亮前重新点亮老灯塔，让船队平安回港。",
        "",
        json!({"quest_type": "main", "objectives": ["找到熄灯的原因", "取回备用的灯芯石", "把灯芯石装回灯室"]}),
    )?;
    let lamp = item(w, &sid, ScriptItemKind::Item, "备用灯芯石", None, "莫伦年轻时藏在潮洞里的另一块灯芯石。", "", json!({"value": 500, "rarity": "rare"}))?;
    item(
        w,
        &sid,
        ScriptItemKind::PlayerCharacter,
        "苇生",
        None,
        "镇上船坞的学徒船匠，手巧、能爬高，修过灯塔的铜框。",
        "与主线的关联：苇生的师父当年帮莫伦造过灯室，苇生知道灯室的构造；莫伦欠苇生师父一个人情。",
        json!({
            "identity": "学徒船匠", "personality": "踏实、话少、遇事不慌",
            "abilities": ["修理", "攀爬", "察觉"],
            "backstory": {"origin": "雾灯镇船坞边长大", "upbringing": "父母出海失事后被老船匠收养", "occupation": "船坞学徒", "key_events": ["十二岁时在风暴里救起过落水的阿澜"], "relations": ["阿澜：一起长大的朋友", "莫伦：师父的老朋友"]}
        }),
    )?;
    item(
        w,
        &sid,
        ScriptItemKind::PlayerCharacter,
        "柯拉",
        None,
        "刚到雾灯镇的流浪歌手，会说话、会打听，背着一把旧琴。",
        "与主线的关联：柯拉唱的一首老歌里提到「蓝石沉睡的洞」，正是潮洞；她自己并不知道歌的来历。",
        json!({
            "identity": "流浪歌手", "personality": "好奇、乐观、嘴甜",
            "abilities": ["说服", "表演", "打听消息"],
            "backstory": {"origin": "南方的港口城市", "upbringing": "跟着戏班长大", "occupation": "四处卖唱", "key_events": ["从一位老水手那里学会了关于蓝石的歌"], "relations": ["苏桦：今天刚请她晚上去互助会唱歌"]}
        }),
    )?;
    item(w, &sid, ScriptItemKind::Ending, "灯亮了", None, "蓝光重新照亮雾湾，船队一艘接一艘平安回港。", "", json!({}))?;

    // 角色
    for (nid, role, secret) in [(&keeper, "守塔人", "灯芯石来自潮洞，海妖想把它拿回去"), (&mayor, "委托人", "儿子在船队里"), (&lan, "向导", "")] {
        let c = NewCastMember { script_id: sid.clone(), node_id: nid.clone(), role: role.into(), motive: String::new(), secret: secret.into(), relation_to_players: String::new(), payload: json!({}) };
        w.write(&Author::User, "示例剧本", |x| x.add_cast(c))?;
    }

    // 剧情图：码头 → 老灯塔 → 潮洞 → 灯亮了
    let mk = |w: &mut World, kind: PlotNodeKind, title: &str, text: &str, scene: Option<&str>| -> Result<String> {
        let mut n = NewPlotNode::new(&sid, kind, title);
        n.player_text = text.into();
        n.scene_ref = scene.map(str::to_string);
        Ok(w.write(&Author::User, "示例剧情", |x| x.create_plot_node(n))?.0.id)
    };
    let p1 = mk(w, PlotNodeKind::Start, "码头", "码头上挤满了焦急的镇民，北边的灯塔一片漆黑。", Some(&town))?;
    let p2 = mk(w, PlotNodeKind::Scene, "老灯塔", "灯室里一片漆黑，莫伦坐在碎掉的灯芯石旁边。", Some(&tower))?;
    let p3 = mk(w, PlotNodeKind::Scene, "潮洞", "退潮后的海蚀洞，石台上有蓝光。", Some(&cave))?;
    let p4 = mk(w, PlotNodeKind::Ending, "灯亮了", "蓝光重新照亮雾湾。", Some(&tower))?;
    let edge = |w: &mut World, a: &str, b: &str, label: &str, effects: serde_json::Value| -> Result<()> {
        let mut e = NewPlotEdge::new(&sid, a, b);
        e.label = label.into();
        e.effects = serde_json::from_value(effects)?;
        w.write(&Author::User, "示例剧情", |x| x.create_plot_edge(e))?;
        Ok(())
    };
    edge(w, &p1, &p2, "答应帮忙，去灯塔看看", json!([{"type": "set_quest_state", "quest": quest, "state": "active"}]))?;
    edge(w, &p2, &p3, "去潮洞取备用灯芯石", json!([]))?;
    edge(w, &p3, &p4, "带着灯芯石回到灯室", json!([{"type": "set_quest_state", "quest": quest, "state": "done"}]))?;
    let _ = lamp;
    Ok(sid)
}
