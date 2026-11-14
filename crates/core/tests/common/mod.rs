#![allow(dead_code)]

use std::path::PathBuf;

use chronicle_core::*;
use tempfile::TempDir;

pub struct Env {
    pub dir: TempDir,
    pub world: World,
}

impl Env {
    pub fn path(&self) -> PathBuf {
        self.world.path().to_path_buf()
    }
    pub fn backups(&self) -> PathBuf {
        self.dir.path().join("backups")
    }
}

pub fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let world = World::create(&dir.path().join("worlds/test.cgw"), &dir.path().join("backups"), "测试世界").unwrap();
    Env { dir, world }
}

pub fn ai() -> Author {
    Author::Ai { job_id: "job1".into() }
}

pub fn user() -> Author {
    Author::User
}

/// 便捷：以用户身份新建节点。
pub fn mk(world: &mut World, n: NewNode) -> Node {
    world.write(&user(), "test", |w| w.create_node(n)).unwrap().0
}

pub fn get(world: &World, id: &str) -> Option<Node> {
    chronicle_core::nodes::get_node(world.conn(), id).unwrap()
}
