//! 生成管线全流程测试。
//! 注意：这里的"模型"是脚本化的 mock，按预设顺序返回固定文本，只验证管线逻辑
//! （上下文、解析修正、变更集、失败恢复），不代表任何真实模型的生成质量。

mod common;
use common::Scripted;

use chronicle_core::changeset::{self, ChangesetStatus, Mode};
use chronicle_core::jobs::{self, JobStatus};
use chronicle_core::*;
use chronicle_gen::runner::{self, GenOptions};
use chronicle_gen::{Task, Templates};
use chronicle_llm::*;
use serde_json::json;

fn env() -> (tempfile::TempDir, World) {
    let dir = tempfile::tempdir().unwrap();
    let w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "雾港世界").unwrap();
    (dir, w)
}

fn user() -> Author {
    Author::User
}

fn mk(w: &mut World, n: NewNode) -> Node {
    w.write(&user(), "t", |x| x.create_node(n)).unwrap().0
}

fn opts() -> GenOptions {
    GenOptions { stream: false, ..Default::default() }
}

fn outline_json() -> serde_json::Value {
    json!({
        "title": "雾港失踪案",
        "genre": "悬疑",
        "tone": "阴郁",
        "premise": "货船在雾中消失",
        "background": "雾港是走私者的天堂",
        "core_setting": "船被码头老板卖给了幽灵船长",
        "opening": {"player_text": "你们在酒馆收到一封求助信", "gm_text": "信是码头老板伪造的"},
        "chapters": [
            {"title": "第一章 迷雾", "player_text": "调查码头", "gm_text": "线索指向灯塔"},
            {"title": "第二章 灯塔", "player_text": "前往灯塔", "gm_text": "幽灵船长现身"}
        ],
        "quests": [
            {"title": "找到货船", "quest_type": "main", "objectives": ["调查码头"], "chapter": "第一章 迷雾"},
            {"title": "老水手的遗愿", "quest_type": "side"},
            {"title": "灯塔的秘密", "quest_type": "hidden", "gm_text": "灯塔看守是共犯"}
        ],
        "npcs": [
            {"name": "码头老板", "role": "委托人", "secret": "就是他卖的船"},
            {"name": "幽灵船长", "role": "反派", "identity": "传说中的海盗", "motivation": "复仇", "secret": "其实是人类"}
        ],
        "endings": [
            {"title": "真相大白", "condition": "揭穿码头老板"},
            {"title": "葬身雾海", "condition": "被幽灵船长击败"}
        ]
    })
}

fn outline_task() -> Task {
    Task::Outline {
        idea: "雾中消失的货船".into(),
        title: None,
        genre: "悬疑".into(),
        tone: String::new(),
        player_count: 4,
        duration_hours: 4.0,
        rules: String::new(),
        use_world: true,
    }
}

#[test]
fn outline_generates_reviewable_changeset_using_existing_world() {
    let (_d, mut w) = env();
    let boss = mk(&mut w, NewNode::new(NodeKind::Character, "码头老板"));
    let mut law = NewNode::new(NodeKind::MagicSystem, "潮汐法则").canon(CanonLevel::Core);
    law.summary = "月相决定魔法强弱".into();
    mk(&mut w, law);

    let client = Scripted::ok(outline_json());
    let t = Templates::builtin();
    let out = runner::run(&mut w, &client, outline_task(), &t, None, &opts(), &CancelToken::new()).unwrap();
    let cs = out.result.unwrap();
    assert!(cs.validation.ok, "{:?}", cs.validation);
    assert_eq!(cs.status, ChangesetStatus::Pending);
    assert!(scripts::list_scripts(w.conn()).unwrap().is_empty(), "确认前不写入");
    assert!(cs.validation.issues.iter().any(|i| i.code == "gen.reused_npc"));

    // 提示词包含世界核心设定与已有名称；服务未声明支持结构化输出时 schema 写在系统提示中。
    let req = &client.requests()[0];
    assert!(matches!(req.format, OutputFormat::Text));
    assert!(req.messages[0].content.contains("JSON Schema"));
    assert!(req.messages[1].content.contains("潮汐法则"));
    assert!(req.messages[1].content.contains("码头老板"));

    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(job.changeset_id.as_deref(), Some(cs.id.as_str()));
    assert!(job.raw_output.unwrap().contains("雾港失踪案"));

    changeset::apply(&mut w, &cs.id, None, &user()).unwrap();
    let s = &scripts::list_scripts(w.conn()).unwrap()[0];
    assert_eq!(s.title, "雾港失踪案");
    assert_eq!(s.core_setting, "船被码头老板卖给了幽灵船长");
    assert_eq!(scripts::items_by_kind(w.conn(), &s.id, ScriptItemKind::Chapter).unwrap().len(), 2);
    let quests = scripts::items_by_kind(w.conn(), &s.id, ScriptItemKind::Quest).unwrap();
    assert_eq!(quests.len(), 3);
    let main = quests.iter().find(|q| q.title == "找到货船").unwrap();
    assert!(main.parent_id.is_some(), "主线任务挂在第一章下");
    assert_eq!(quests.iter().find(|q| q.title == "灯塔的秘密").unwrap().payload["quest_type"], "hidden");
    let ending = &scripts::items_by_kind(w.conn(), &s.id, ScriptItemKind::Ending).unwrap()[0];
    assert!(ending.gm_text.contains("达成条件"));
    let cast = scripts::cast_of(w.conn(), &s.id).unwrap();
    assert_eq!(cast.len(), 2);
    assert!(cast.iter().any(|c| c.node_id == boss.id && c.secret == "就是他卖的船"));
    let ghost = nodes::find_by_name(w.conn(), "幽灵船长", None).unwrap();
    assert_eq!(ghost.len(), 1);
    assert_eq!(ghost[0].payload["secrets"], json!(["其实是人类"]));
    assert_eq!(nodes::get_node(w.conn(), ghost[0].parent_id.as_deref().unwrap()).unwrap().unwrap().title, "人物");
    assert_eq!(ghost[0].source, format!("ai:{}", out.job_id));
}

#[test]
fn json_schema_mode_used_when_supported_and_falls_back_on_400() {
    let (_d, mut w) = env();
    let caps = Capabilities { chat: true, json_schema: true, ..Default::default() };
    let client = Scripted::ok(outline_json());
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), Some(&caps), &opts(), &CancelToken::new()).unwrap();
    assert!(out.result.is_ok());
    let req = &client.requests()[0];
    assert!(matches!(req.format, OutputFormat::JsonSchema { .. }));
    assert!(!req.messages[0].content.contains("JSON Schema"));

    let mut client = Scripted::ok(outline_json());
    client.reject_schema = true;
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), Some(&caps), &opts(), &CancelToken::new()).unwrap();
    assert!(out.result.is_ok());
    let reqs = client.requests();
    assert_eq!(reqs.len(), 2);
    assert!(matches!(reqs[1].format, OutputFormat::Text));
    assert!(reqs[1].messages[0].content.contains("JSON Schema"));
    assert!(!out.requests[0].ok);
}

#[test]
fn bad_output_is_repaired_once() {
    let (_d, mut w) = env();
    let client = Scripted::new(vec![
        Ok("<think>想一想</think>好的，这是大纲：{\"title\": \"缺字段\"".into()),
        Ok(format!("```json\n{}\n```", outline_json())),
    ]);
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    assert!(out.result.is_ok());
    let reqs = client.requests();
    assert_eq!(reqs.len(), 2);
    let repair = &reqs[1].messages;
    assert_eq!(repair.len(), 4);
    assert!(repair[3].content.contains("无法使用"));
    assert!(jobs::get_job(w.conn(), &out.job_id).unwrap().note.contains("修正"));
}

#[test]
fn failure_leaves_world_untouched_and_retry_works() {
    let (_d, mut w) = env();
    mk(&mut w, NewNode::new(NodeKind::City, "雾港"));
    let commits_before = w.commits(100, 0).unwrap().len();
    // 两次都返回结构错误（chapters 为空）。
    let bad = json!({"title": "x", "opening": {}, "chapters": [], "quests": [], "npcs": [], "endings": []}).to_string();
    let client = Scripted::new(vec![Ok(bad.clone()), Ok(bad)]);
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    assert!(matches!(out.result, Err(chronicle_gen::GenError::Output(_))));
    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    assert_eq!(job.status, JobStatus::Failed);
    assert!(job.raw_output.is_some(), "失败时保留原始输出");
    assert!(job.error.unwrap().contains("chapters"));
    assert_eq!(w.commits(100, 0).unwrap().len(), commits_before);
    assert!(changeset::list_changesets(w.conn(), None).unwrap().is_empty());

    // 模型服务不可用也一样。
    let down = Scripted::new(vec![Err(LlmError::Transport("connection refused".into()))]);
    let out2 = runner::run(&mut w, &down, outline_task(), &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    assert!(out2.result.is_err());
    assert_eq!(jobs::get_job(w.conn(), &out2.job_id).unwrap().status, JobStatus::Failed);

    // 重试原任务：参数不变，尝试次数 +1。
    let good = Scripted::ok(outline_json());
    let r = runner::retry(&mut w, &good, &out.job_id, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    assert!(r.result.is_ok());
    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    assert_eq!((job.status, job.attempts), (JobStatus::Succeeded, 2));
    // 成功的任务不能再重试。
    assert!(runner::retry(&mut w, &good, &out.job_id, &Templates::builtin(), None, &opts(), &CancelToken::new()).is_err());
}

#[test]
fn outline_without_quests_is_rejected_and_repaired() {
    // 真实联调中模型曾省略 quests；现在该字段必填，缺失时要求模型修正。
    let (_d, mut w) = env();
    let mut missing = outline_json();
    missing.as_object_mut().unwrap().remove("quests");
    let client = Scripted::new(vec![Ok(missing.to_string()), Ok(outline_json().to_string())]);
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    assert!(out.result.is_ok());
    let reqs = client.requests();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].messages[3].content.contains("quests"));
    let schema = serde_json::to_string(&Task::schema(&outline_task()).1).unwrap();
    assert!(schema.contains("\"required\"") && schema.contains("\"quests\""));
}

#[test]
fn cancelled_job_is_marked_cancelled() {
    let (_d, mut w) = env();
    let cancel = CancelToken::new();
    cancel.cancel();
    let client = Scripted::ok(outline_json());
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &opts(), &cancel).unwrap();
    assert!(matches!(out.result, Err(chronicle_gen::GenError::Cancelled)));
    assert_eq!(jobs::get_job(w.conn(), &out.job_id).unwrap().status, JobStatus::Cancelled);
}

/// 先生成并应用大纲，返回剧本 ID。
fn applied_script(w: &mut World) -> String {
    let client = Scripted::ok(outline_json());
    let out = runner::run(w, &client, outline_task(), &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    let cs = out.result.unwrap();
    changeset::apply(w, &cs.id, None, &user()).unwrap();
    scripts::list_scripts(w.conn()).unwrap()[0].id.clone()
}

#[test]
fn expand_chapter_appends_scenes_and_clues() {
    let (_d, mut w) = env();
    let harbor = mk(&mut w, NewNode::new(NodeKind::City, "雾港"));
    let sid = applied_script(&mut w);
    let ch = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::Chapter).unwrap()[0].clone();
    let client = Scripted::ok(json!({
        "scenes": [
            {"title": "码头夜色", "player_text": "雾气弥漫", "gm_text": "守卫收了贿赂", "location": "雾港", "npcs": ["码头老板", "神秘乞丐"]},
            {"title": "仓库", "player_text": "空荡的仓库"}
        ],
        "clues": [{"title": "湿脚印", "found_in": "仓库", "gm_text": "通向灯塔"}, {"title": "旧账本"}],
        "items": [{"title": "铜钥匙"}],
        "key_events": [{"title": "守卫换岗", "trigger": "午夜"}],
        "twists": [{"title": "委托人说谎", "gm_text": "码头老板是幕后黑手"}]
    }));
    let task = Task::ExpandChapter { script_id: sid.clone(), chapter_id: ch.id.clone(), guidance: String::new() };
    let out = runner::run(&mut w, &client, task, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    let cs = out.result.unwrap();
    assert_eq!(cs.mode, Mode::Append);
    assert!(cs.validation.issues.iter().any(|i| i.code == "gen.unknown_npc"));
    // 提示词带有剧本大纲与要展开的章节。
    assert!(client.requests()[0].messages[1].content.contains("第一章 迷雾"));
    changeset::apply(&mut w, &cs.id, None, &user()).unwrap();

    let kids = scripts::item_children(w.conn(), &sid, Some(&ch.id)).unwrap();
    let scene = kids.iter().find(|i| i.title == "码头夜色").unwrap();
    assert_eq!(scene.payload["location_id"], json!(harbor.id));
    assert_eq!(scene.payload["npc_names"], json!(["神秘乞丐"]));
    assert_eq!(scene.payload["npc_ids"].as_array().unwrap().len(), 1);
    let warehouse = kids.iter().find(|i| i.title == "仓库").unwrap();
    let under_warehouse = scripts::item_children(w.conn(), &sid, Some(&warehouse.id)).unwrap();
    assert_eq!(under_warehouse[0].title, "湿脚印");
    assert!(kids.iter().any(|i| i.title == "旧账本" && i.kind == ScriptItemKind::Clue));
    assert!(kids.iter().any(|i| i.kind == ScriptItemKind::KeyEvent && i.payload["trigger"] == "午夜"));
    // 已有条目未被修改。
    assert_eq!(scripts::get_item(w.conn(), &ch.id).unwrap().unwrap().rev, 1);
}

#[test]
fn regenerate_only_touches_selected_unlocked_items() {
    let (_d, mut w) = env();
    let sid = applied_script(&mut w);
    let chs = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::Chapter).unwrap();
    let (a, b) = (chs[0].clone(), chs[1].clone());
    let opening = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::Opening).unwrap()[0].clone();
    w.write(&user(), "lock", |x| x.update_script_item(&b.id, ScriptItemPatch { locked: Some(true), ..Default::default() }, None)).unwrap();

    let client = Scripted::ok(json!({"items": [
        {"id": a.id, "title": "第一章 浓雾", "player_text": "新的描述", "gm_text": "新的真相"},
        {"id": opening.id, "title": "偷偷改开场", "player_text": "x"}
    ]}));
    let task = Task::Regenerate { script_id: sid.clone(), item_ids: vec![a.id.clone(), b.id.clone()], instruction: "更恐怖".into() };
    let out = runner::run(&mut w, &client, task, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    let cs = out.result.unwrap();
    let codes: Vec<_> = cs.validation.issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"gen.skipped_locked") && codes.contains(&"gen.unexpected_item"), "{codes:?}");
    assert_eq!(cs.ops.len(), 1);
    // 锁定条目不会出现在提示词的待重写列表中。
    let prompt = &client.requests()[0].messages[1].content;
    assert!(prompt.contains(&a.id) && !prompt.contains(&format!("(id={})", b.id)));
    changeset::apply(&mut w, &cs.id, None, &user()).unwrap();
    assert_eq!(scripts::get_item(w.conn(), &a.id).unwrap().unwrap().title, "第一章 浓雾");
    assert_eq!(scripts::get_item(w.conn(), &b.id).unwrap().unwrap().title, "第二章 灯塔");
    assert_eq!(scripts::get_item(w.conn(), &opening.id).unwrap().unwrap().title, "开场");
}

#[test]
fn regenerate_conflicts_if_item_edited_during_generation() {
    let (_d, mut w) = env();
    let sid = applied_script(&mut w);
    let a = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::Chapter).unwrap()[0].clone();
    let client = Scripted::ok(json!({"items": [{"id": a.id, "title": "AI 版本"}]}));
    let task = Task::Regenerate { script_id: sid, item_ids: vec![a.id.clone()], instruction: String::new() };
    let cs = runner::run(&mut w, &client, task, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap().result.unwrap();
    w.write(&user(), "manual", |x| x.update_script_item(&a.id, ScriptItemPatch { title: Some("用户版本".into()), ..Default::default() }, None)).unwrap();
    assert!(changeset::apply(&mut w, &cs.id, None, &user()).is_err());
    assert_eq!(scripts::get_item(w.conn(), &a.id).unwrap().unwrap().title, "用户版本");
}

#[test]
fn extend_only_adds() {
    let (_d, mut w) = env();
    let sid = applied_script(&mut w);
    let before: Vec<_> = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::Chapter).unwrap();
    let client = Scripted::ok(json!({
        "chapters": [{"title": "第三章 归航"}],
        "scenes": [{"chapter": "第三章 归航", "title": "海上风暴"}, {"chapter": "第一章 迷雾", "title": "补充场景"}, {"chapter": "不存在", "title": "孤儿场景"}],
        "quests": [{"title": "护送幸存者", "quest_type": "side", "chapter": "第三章 归航"}],
        "npcs": [{"name": "幸存的水手", "role": "证人"}],
        "endings": [{"title": "平安归航", "condition": "护送成功"}]
    }));
    let task = Task::Extend { script_id: sid.clone(), instruction: "增加返航章节".into() };
    let cs = runner::run(&mut w, &client, task, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap().result.unwrap();
    assert_eq!(cs.mode, Mode::Append);
    assert!(cs.ops.iter().all(|o| o.is_create()));
    assert!(cs.validation.issues.iter().any(|i| i.code == "gen.unknown_chapter"));
    changeset::apply(&mut w, &cs.id, None, &user()).unwrap();
    let after = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::Chapter).unwrap();
    assert_eq!(after.len(), 3);
    for b in &before {
        assert_eq!(scripts::get_item(w.conn(), &b.id).unwrap().unwrap().rev, 1, "已有章节未被修改");
    }
    let first = &before[0];
    assert!(scripts::item_children(w.conn(), &sid, Some(&first.id)).unwrap().iter().any(|i| i.title == "补充场景"));
    assert_eq!(scripts::cast_of(w.conn(), &sid).unwrap().len(), 3);
}

#[test]
fn npcs_update_existing_respect_protection_and_add_new() {
    let (_d, mut w) = env();
    let sid = applied_script(&mut w);
    let boss = nodes::find_by_name(w.conn(), "码头老板", None).unwrap()[0].clone();
    let ghost = nodes::find_by_name(w.conn(), "幽灵船长", None).unwrap()[0].clone();
    w.write(&user(), "core", |x| x.update_node(&ghost.id, NodePatch { canon_level: Some(CanonLevel::Core), ..Default::default() }, None)).unwrap();
    let client = Scripted::ok(json!({"npcs": [
        {"name": "码头老板", "aliases": ["老鲍"], "personality": "贪婪", "secrets": ["欠了赌债"], "knowledge": ["货船去向"], "summary": "雾港码头的主人"},
        {"name": "灯塔看守", "identity": "老人", "role": "共犯", "secrets": ["收了钱"]},
        {"name": "多余的人"}
    ]}));
    let task = Task::Npcs { script_id: sid.clone(), node_ids: vec![boss.id.clone(), ghost.id.clone()], new_count: 1, guidance: String::new() };
    let cs = runner::run(&mut w, &client, task, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap().result.unwrap();
    let codes: Vec<_> = cs.validation.issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"gen.skipped_protected") && codes.contains(&"gen.ignored_npc"), "{codes:?}");
    assert!(cs.validation.ok);
    changeset::apply(&mut w, &cs.id, None, &user()).unwrap();
    let boss2 = nodes::get_node(w.conn(), &boss.id).unwrap().unwrap();
    assert_eq!(boss2.aliases, vec!["老鲍".to_string()]);
    assert_eq!(boss2.payload["personality"], "贪婪");
    assert_eq!(boss2.payload["knowledge"], json!(["货船去向"]));
    assert_eq!(boss2.summary, "雾港码头的主人");
    assert_eq!(nodes::get_node(w.conn(), &ghost.id).unwrap().unwrap().rev, ghost.rev + 1, "核心人物只被用户改过一次");
    assert_eq!(nodes::find_by_name(w.conn(), "灯塔看守", None).unwrap().len(), 1);
    assert!(nodes::find_by_name(w.conn(), "多余的人", None).unwrap().is_empty());
    assert_eq!(scripts::cast_of(w.conn(), &sid).unwrap().len(), 3);
}

#[test]
fn small_budget_is_reported() {
    let (_d, mut w) = env();
    for i in 0..60 {
        let mut n = NewNode::new(NodeKind::Faction, format!("势力{i}")).canon(CanonLevel::Core);
        n.summary = "很长的设定说明".repeat(20);
        mk(&mut w, n);
    }
    let client = Scripted::ok(outline_json());
    let o = GenOptions { context_budget: 300, stream: false, ..Default::default() };
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &o, &CancelToken::new()).unwrap();
    assert!(out.result.is_ok());
    assert!(jobs::get_job(w.conn(), &out.job_id).unwrap().note.contains("省略"));
}

#[test]
fn template_override_changes_version() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("outline.j2"), "自定义：{{ idea }}").unwrap();
    let t = Templates::load(Some(dir.path())).unwrap();
    assert_eq!(t.overridden, vec!["outline".to_string()]);
    assert_ne!(t.version, Templates::builtin().version);
    std::fs::write(dir.path().join("outline.j2"), "{% if %}").unwrap();
    assert!(Templates::load(Some(dir.path())).is_err());
}

#[test]
fn truncated_output_retries_with_thinking_disabled() {
    let (_d, mut w) = env();
    let client = Scripted::new(vec![Ok("__LENGTH__<think>想了很久很久……".into()), Ok(outline_json().to_string())]);
    let out = runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap();
    assert!(out.result.is_ok());
    let reqs = client.requests();
    assert_eq!(reqs[0].thinking, None, "第一次按模型默认");
    assert_eq!(reqs[1].thinking, Some(false), "截断后修正时关闭思考");
    assert!(reqs[1].messages[3].content.contains("截断"));
    assert!(jobs::get_job(w.conn(), &out.job_id).unwrap().note.contains("关闭了思考"));

    // 设置为关闭思考时，每次请求都带上。
    let client = Scripted::ok(outline_json());
    let o = GenOptions { stream: false, thinking: Some(false), ..Default::default() };
    runner::run(&mut w, &client, outline_task(), &Templates::builtin(), None, &o, &CancelToken::new()).unwrap();
    assert_eq!(client.requests()[0].thinking, Some(false));
}
