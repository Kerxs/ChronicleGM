//! Chronicle GM 核心：数据模型、持久化、迁移、变更集、修订与撤销、搜索、导入导出、战役状态。
//!
//! 所有业务规则都在这里（以及 `chronicle-rules`）实现，界面层只调用这些接口。

pub mod archive;
pub mod backup;
pub mod bundle;
pub mod campaign;
pub mod combat;
pub mod changeset;
pub mod checks;
pub mod consistency;
pub mod console;
pub mod db;
pub mod demo;
pub mod director;
pub mod entity;
pub mod error;
pub mod images;
pub mod integrity;
pub mod investigation;
pub mod jobs;
pub mod library;
pub mod model;
pub mod network;
pub mod npcsim;
pub mod nodes;
pub mod payload;
pub mod pc;
pub mod plot;
pub mod plot_analysis;
pub mod profile;
pub mod quests;
pub mod relations;
pub mod rpg;
pub mod schedule;
pub mod script_admin;
pub mod spoiler;
pub mod story;
pub mod scripts;
pub mod search;
pub mod sim;
pub mod timeline;
pub mod util;
pub mod versions;
pub mod worldsim;
pub mod world;

pub use error::{CoreError, Result};
pub use library::{Library, WorldInfo};
pub use model::*;
pub use world::{CommitInfo, World, Writer};

pub use chronicle_rules as rules;
pub use rusqlite;
