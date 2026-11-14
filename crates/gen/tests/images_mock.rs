//! 参考图片：生成任务带图片（mock 模型，只验证请求里确实带了图片、能力检查与重试，不代表真实模型的看图效果）。

mod common;

use chronicle_core::jobs::{self, JobStatus};
use chronicle_core::*;
use chronicle_gen::runner::{self, GenOptions};
use chronicle_gen::{Task, Templates};
use chronicle_llm::{Capabilities, CancelToken};
use common::Scripted;
use serde_json::json;

const RED: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGN4ICBAEmIY1TCqQWDYagAA6ggAEK2bvLwAAAAASUVORK5CYII=";

fn env() -> (tempfile::TempDir, World) {
    let dir = tempfile::tempdir().unwrap();
    let w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "测试").unwrap();
    (dir, w)
}

fn opts() -> GenOptions {
    GenOptions { stream: false, ..Default::default() }
}

fn caps(vision: bool) -> Capabilities {
    Capabilities { chat: true, json_schema: true, vision, tested_at: 1, ..Default::default() }
}

fn expand_reply() -> Scripted {
    Scripted::ok(json!({ "nodes": [{"name": "灯塔下的小屋", "kind": "site", "summary": "图里那座小屋"}], "relations": [] }))
}

#[test]
fn reference_images_are_sent_with_the_prompt_and_kept_for_retry() {
    let (_d, mut w) = env();
    let city = w.write(&Author::User, "t", |x| x.create_node(NewNode::new(NodeKind::City, "雾港"))).unwrap().0;
    // 节点上已有一张图片：用 node-image: 引用。
    let png = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(RED.split_once(",").unwrap().1).unwrap()
    };
    let img = images::add_image(w.conn(), &city.id, &png, 16, 16, "雾港全景").unwrap();
    let client = expand_reply();
    let task = Task::ExpandNode { node_id: city.id.clone(), count: 1, kinds: vec![], guidance: String::new() };
    let out = runner::run_with_images(&mut w, &client, task, &Templates::builtin(), Some(&caps(true)), &opts(), &CancelToken::new(), vec![RED.into(), format!("node-image:{}", img.id)]).unwrap();
    assert!(out.result.is_ok(), "{:?}", out.result.err());
    let req = &client.requests()[0];
    let user = &req.messages[1];
    assert_eq!(user.images.len(), 2, "两张图都在请求里");
    assert!(user.images.iter().all(|u| u.starts_with("data:image/png;base64,")), "节点图片引用换成了图片数据");
    assert!(user.content.contains("【参考图片】附带了 2 张参考图片"));
    // 发给服务的格式：内容数组，第一项文字，后面是图片。
    let wire = user.to_wire();
    assert_eq!(wire["content"][0]["type"], "text");
    assert_eq!(wire["content"][1]["type"], "image_url");
    assert!(wire["content"][2]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png"));
    // 系统消息没有图片，仍是普通字符串。
    assert!(req.messages[0].to_wire()["content"].is_string());
    // 图片随任务参数保存。
    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    assert_eq!(job.params["reference_images"].as_array().unwrap().len(), 2);
}

#[test]
fn reference_images_require_a_vision_capable_service() {
    let (_d, mut w) = env();
    let city = w.write(&Author::User, "t", |x| x.create_node(NewNode::new(NodeKind::City, "雾港"))).unwrap().0;
    let client = expand_reply();
    let task = Task::ExpandNode { node_id: city.id.clone(), count: 1, kinds: vec![], guidance: String::new() };
    let err = runner::run_with_images(&mut w, &client, task.clone(), &Templates::builtin(), Some(&caps(false)), &opts(), &CancelToken::new(), vec![RED.into()]).err().expect("应当拒绝");
    assert!(err.to_string().contains("看图"), "{err}");
    assert!(client.requests().is_empty(), "没有发出任何请求");
    // 任务记为失败并写明原因（可在生成任务页看到）。
    let failed = jobs::list_jobs(w.conn(), None, 10).unwrap().into_iter().find(|j| j.status == JobStatus::Failed).unwrap();
    assert!(failed.error.unwrap_or_default().contains("看图"));
    // 不带图片时照常可以生成。
    let out = runner::run_with_images(&mut w, &client, task, &Templates::builtin(), Some(&caps(false)), &opts(), &CancelToken::new(), vec![]).unwrap();
    assert!(out.result.is_ok());
    assert!(client.requests()[0].messages[1].images.is_empty());
}

#[test]
fn bad_reference_images_are_rejected_before_anything_runs() {
    let (_d, mut w) = env();
    let city = w.write(&Author::User, "t", |x| x.create_node(NewNode::new(NodeKind::City, "雾港"))).unwrap().0;
    let client = expand_reply();
    let task = Task::ExpandNode { node_id: city.id.clone(), count: 1, kinds: vec![], guidance: String::new() };
    assert!(runner::run_with_images(&mut w, &client, task.clone(), &Templates::builtin(), Some(&caps(true)), &opts(), &CancelToken::new(), vec!["http://example.com/a.png".into()]).is_err());
    assert!(runner::run_with_images(&mut w, &client, task, &Templates::builtin(), Some(&caps(true)), &opts(), &CancelToken::new(), vec![RED.into(); 7]).err().expect("应当拒绝").to_string().contains("最多"));
    assert!(client.requests().is_empty());
}
