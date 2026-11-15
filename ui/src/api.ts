// 与 Rust 后端通信的唯一入口。界面不包含业务规则，只调用这些命令。
// 类型来自 ./bindings（由 ts-rs 从 Rust 自动生成，不要手改）。
import { invoke } from "@tauri-apps/api/core";
import type { WorldInfo } from "./bindings/WorldInfo";
import type { NodeImage } from "./bindings/NodeImage";
import type { GameProfile } from "./bindings/GameProfile";
import type { ScriptFinding } from "./bindings/ScriptFinding";
import type { PlayerCharacter } from "./bindings/PlayerCharacter";
import type { GlobalHit } from "./bindings/GlobalHit";
import type { NpcMemory } from "./bindings/NpcMemory";
import type { DeletedCampaign } from "./bindings/DeletedCampaign";
import type { NodeFilter } from "./bindings/NodeFilter";
import type { FieldInfo } from "./bindings/FieldInfo";
import type { SearchConfig } from "./bindings/SearchConfig";
import type { SearchResult } from "./bindings/SearchResult";
import type { MemorySettings } from "./bindings/MemorySettings";
import type { RecentRequest } from "./bindings/RecentRequest";
import type { RunStats } from "./bindings/RunStats";
import type { DeletedScript } from "./bindings/DeletedScript";
import type { ScriptVersion } from "./bindings/ScriptVersion";
import type { VersionDiff } from "./bindings/VersionDiff";
import type { QuestProposal } from "./bindings/QuestProposal";
import type { NewQuest } from "./bindings/NewQuest";
import type { ImageThumb } from "./bindings/ImageThumb";
import type { IllustratedNode } from "./bindings/IllustratedNode";
import type { SceneView } from "./bindings/SceneView";
import type { MemoryAdvice } from "./bindings/MemoryAdvice";
import type { ArchiveEntry } from "./bindings/ArchiveEntry";
import type { CampaignStats } from "./bindings/CampaignStats";
import type { EndingDistribution } from "./bindings/EndingDistribution";
import type { Chapter } from "./bindings/Chapter";
import type { Chronicle } from "./bindings/Chronicle";
import type { SearchFilter } from "./bindings/SearchFilter";
import type { EventHit } from "./bindings/EventHit";
import type { ImportResult } from "./bindings/ImportResult";
import type { CampaignCheck } from "./bindings/CampaignCheck";
import type { Bond } from "./bindings/Bond";
import type { Board } from "./bindings/Board";
import type { HypothesisInput } from "./bindings/HypothesisInput";
import type { QuestView } from "./bindings/QuestView";
import type { NpcStatus } from "./bindings/NpcStatus";
import type { Environment } from "./bindings/Environment";
import type { WorldMap } from "./bindings/WorldMap";
import type { Reachable } from "./bindings/Reachable";
import type { FactionState } from "./bindings/FactionState";
import type { WorldEvent } from "./bindings/WorldEvent";
import type { TravelPlan } from "./bindings/TravelPlan";
import type { Sheet } from "./bindings/Sheet";
import type { GmConsole } from "./bindings/GmConsole";
import type { DirectorNote } from "./bindings/DirectorNote";
import type { DirectorSettings } from "./bindings/DirectorSettings";
import type { RuleSet } from "./bindings/RuleSet";
import type { InvItem } from "./bindings/InvItem";
import type { ItemInfo } from "./bindings/ItemInfo";
import type { GrowthEntry } from "./bindings/GrowthEntry";
import type { Shop } from "./bindings/Shop";
import type { Combat } from "./bindings/Combat";
import type { CombatReport } from "./bindings/CombatReport";
import type { CombatAction } from "./bindings/CombatAction";
import type { EnemySpec } from "./bindings/EnemySpec";
import type { Mood } from "./bindings/Mood";
import type { Network } from "./bindings/Network";
import type { NetworkQuery } from "./bindings/NetworkQuery";
import type { BondChange } from "./bindings/BondChange";
import type { BondSummary } from "./bindings/BondSummary";
import type { Checkpoint } from "./bindings/Checkpoint";
import type { ForkPoint } from "./bindings/ForkPoint";
import type { VerifyReport } from "./bindings/VerifyReport";
import type { TimelineComparison } from "./bindings/TimelineComparison";
import type { SchedSnapshot } from "./bindings/SchedSnapshot";
import type { ResourceSample } from "./bindings/ResourceSample";
import type { ProcStatus } from "./bindings/ProcStatus";
import type { ModelProfile } from "./bindings/ModelProfile";
import type { MemEstimate } from "./bindings/MemEstimate";
import type { Priority } from "./bindings/Priority";
import type { BackupInfo } from "./bindings/BackupInfo";
import type { IntegrityReport } from "./bindings/IntegrityReport";
import type { RepairReport } from "./bindings/RepairReport";
import type { IncompleteTurn } from "./bindings/IncompleteTurn";
import type { Script } from "./bindings/Script";
import type { ScriptItem } from "./bindings/ScriptItem";
import type { CastMember } from "./bindings/CastMember";
import type { NewScript } from "./bindings/NewScript";
import type { ScriptPatch } from "./bindings/ScriptPatch";
import type { NewScriptItem } from "./bindings/NewScriptItem";
import type { ScriptItemPatch } from "./bindings/ScriptItemPatch";
import type { Changeset } from "./bindings/Changeset";
import type { ChangesetStatus } from "./bindings/ChangesetStatus";
import type { Op } from "./bindings/Op";
import type { Task } from "./bindings/Task";
import type { GenJob } from "./bindings/GenJob";
import type { CommitInfo } from "./bindings/CommitInfo";
import type { Node } from "./bindings/Node";
import type { DiscoveredService } from "./bindings/DiscoveredService";
import type { ProviderConfig } from "./bindings/ProviderConfig";
import type { Capabilities } from "./bindings/Capabilities";
import type { ProviderStats } from "./bindings/ProviderStats";
import type { GgufInfo } from "./bindings/GgufInfo";
import type { NewNode } from "./bindings/NewNode";
import type { NodePatch } from "./bindings/NodePatch";
import type { NodeKind } from "./bindings/NodeKind";
import type { Position } from "./bindings/Position";
import type { Relation } from "./bindings/Relation";
import type { NewRelation } from "./bindings/NewRelation";
import type { RelationPatch } from "./bindings/RelationPatch";
import type { RelationType } from "./bindings/RelationType";
import type { SearchOptions } from "./bindings/SearchOptions";
import type { SearchHit } from "./bindings/SearchHit";
import type { Graph } from "./bindings/Graph";
import type { Finding } from "./bindings/Finding";
import type { WorldVersion } from "./bindings/WorldVersion";
import type { BundleManifest } from "./bindings/BundleManifest";
import type { ImportReport } from "./bindings/ImportReport";
import type { ConflictStrategy } from "./bindings/ConflictStrategy";
import type { PlotNode } from "./bindings/PlotNode";
import type { PlotEdge } from "./bindings/PlotEdge";
import type { NewPlotNode } from "./bindings/NewPlotNode";
import type { NewPlotEdge } from "./bindings/NewPlotEdge";
import type { PlotNodePatch } from "./bindings/PlotNodePatch";
import type { PlotReport } from "./bindings/PlotReport";
import type { SimState } from "./bindings/SimState";
import type { SimStep } from "./bindings/SimStep";
import type { CheckMode } from "./bindings/CheckMode";
import type { EdgeOption } from "./bindings/EdgeOption";
import type { Campaign } from "./bindings/Campaign";
import type { CampaignState } from "./bindings/CampaignState";
import type { EventEntry } from "./bindings/EventEntry";
import type { TransitionOutcome } from "./bindings/TransitionOutcome";
import type { Condition } from "./bindings/Condition";
import type { Effect } from "./bindings/Effect";
import type { TurnResult } from "./bindings/TurnResult";
import type { SessionSummary } from "./bindings/SessionSummary";
import type { CampaignEntity } from "./bindings/CampaignEntity";

export interface AppInfo {
  version: string;
  data_dir: string;
  template_version: string;
  templates_overridden: string[];
}

export interface WorldOverview {
  info: WorldInfo;
  root_nodes: number;
  node_count: number;
  script_count: number;
  pending_changesets: number;
}

export interface CastView {
  cast: CastMember;
  name: string;
  node_rev: number;
}

export interface ScriptView {
  script: Script;
  items: ScriptItem[];
  cast: CastView[];
}

export interface ProviderView {
  config: ProviderConfig;
  has_key: boolean;
  loopback: boolean;
}

export interface ScanResult {
  models: GgufInfo[];
  errors: [string, string][];
  executables: string[];
}

export interface TreeNode {
  node: Node;
  child_count: number;
}

export interface RelationView {
  relation: Relation;
  other_id: string;
  other_title: string;
  other_kind: NodeKind;
  outgoing: boolean;
}

export interface NodeDetail {
  node: Node;
  ancestors: Node[];
  relations: RelationView[];
  child_count: number;
}

export interface PlotGraph {
  nodes: PlotNode[];
  edges: PlotEdge[];
}

export interface EdgeUpdate {
  label: string;
  condition: Condition | null;
  effects: Effect[];
  priority: number;
  once: boolean;
}

export interface CampaignStatus {
  campaign: Campaign;
  current: PlotNode | null;
  options: EdgeOption[];
  state: CampaignState;
  events: EventEntry[];
  /** 上次程序意外退出时没有完成的回合。 */
  incomplete: IncompleteTurn | null;
}

export interface CampaignMemory {
  summaries: SessionSummary[];
  facts: string[];
  fact_seqs: number[];
  entities: CampaignEntity[];
}

export interface ProgressEvent {
  world_id: string;
  job_id: string;
  chars: number;
}

export interface FinishedEvent {
  world_id: string;
  job_id: string;
  ok: boolean;
  error: string | null;
  changeset_id: string | null;
}

/** 后端错误以字符串形式返回，这里统一转成 Error。 */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : String(e));
  }
}

export interface RenderStat {
  view: string;
  nodes: number;
  edges: number;
  ms: number;
  at: number;
}

export interface PerfSnapshot {
  sched: SchedSnapshot;
  resources: ResourceSample | null;
  history: ResourceSample[];
  models: ProcStatus[];
  render: RenderStat[];
  voice_note: string;
}

export interface DbProbe {
  file_mb: number;
  wal_mb: number;
  nodes: number;
  events: number;
  timings: [string, number][];
}

export interface NpcView {
  status: NpcStatus;
  mood: Mood | null;
  bond_to_party: Bond | null;
}

export interface RpgOverview {
  sheet: Sheet;
  ac: number;
  weapon: string;
  rules: RuleSet;
  money: number;
  money_text: string;
  inventory: InvItem[];
  equipment: Record<string, string>;
  growth: GrowthEntry[];
  recipes: [ItemInfo, boolean][];
  merchants: { id: string; name: string; shop: Shop }[];
  combat: Combat | null;
  combats: Combat[];
  sell_prices: Record<string, Record<string, number>>;
  next_level_xp: number;
  item_names: Record<string, string>;
}

export interface WorldSimOverview {
  environment: Environment;
  map: WorldMap;
  reachable: Reachable[];
  factions: FactionState[];
  events: WorldEvent[];
}

export interface TravelDone {
  plan: TravelPlan;
  notes: string[];
}

export interface NpcStates {
  time: string;
  minutes: number;
  npcs: NpcView[];
}

export const api = {
  investigationBoard: (worldId: string, campaignId: string, gmView: boolean) => call<Board>("investigation_board", { worldId, campaignId, gmView }),
  boardAddNote: (worldId: string, campaignId: string, title: string, detail: string) => call<string>("board_add_note", { worldId, campaignId, title, detail }),
  boardRemoveNote: (worldId: string, campaignId: string, id: string) => call<void>("board_remove_note", { worldId, campaignId, id }),
  boardLink: (worldId: string, campaignId: string, a: string, b: string, relation: string, note: string) => call<string>("board_link", { worldId, campaignId, a, b, relation, note }),
  boardUnlink: (worldId: string, campaignId: string, id: string) => call<void>("board_unlink", { worldId, campaignId, id }),
  boardSaveHypothesis: (worldId: string, campaignId: string, id: string | null, input: HypothesisInput) => call<string>("board_save_hypothesis", { worldId, campaignId, id, input }),
  boardDeleteHypothesis: (worldId: string, campaignId: string, id: string) => call<void>("board_delete_hypothesis", { worldId, campaignId, id }),
  questList: (worldId: string, campaignId: string, gmView: boolean) => call<QuestView[]>("quest_list", { worldId, campaignId, gmView }),
  npcStates: (worldId: string, campaignId: string) => call<NpcStates>("npc_states", { worldId, campaignId }),
  advanceStoryTime: (worldId: string, campaignId: string, minutes: number) => call<string[]>("advance_story_time", { worldId, campaignId, minutes }),
  voiceSpeak: (worldId: string, speaker: string | null, text: string) => call<{ data_url: string; cached: boolean; seconds: number; ms: number }>("voice_speak", { worldId, speaker, text }),
  voiceStatus: () => call<{ configured: boolean; base_url: string; healthy: boolean; cloud: boolean; problem: string | null; cache_files: number; cache_mb: number; queued: number; narrator: { instructions: string; voice_id: string; seed: number | null } }>("voice_status"),
  voiceSetNarrator: (voice: { instructions: string; voice_id: string; seed: number | null }) => call<void>("voice_set_narrator", { voice }),
  voiceClearCache: () => call<number>("voice_clear_cache"),
  gmConsole: (worldId: string, campaignId: string) => call<GmConsole>("gm_console", { worldId, campaignId }),
  consoleApply: (worldId: string, campaignId: string, effects: Effect[], reason: string) => call<number>("console_apply", { worldId, campaignId, effects, reason }),
  directorAnalyze: (worldId: string, campaignId: string) => call<DirectorNote>("director_analyze", { worldId, campaignId }),
  directorSetStatus: (worldId: string, campaignId: string, noteId: string, suggestionId: string, status: string) => call<void>("director_set_status", { worldId, campaignId, noteId, suggestionId, status }),
  directorSetSettings: (worldId: string, campaignId: string, settings: DirectorSettings) => call<void>("director_set_settings", { worldId, campaignId, settings }),
  rpgOverview: (worldId: string, campaignId: string) => call<RpgOverview>("rpg_overview", { worldId, campaignId, actor: null }),
  rpgSaveSheet: (worldId: string, campaignId: string, sheet: Sheet) => call<void>("rpg_save_sheet", { worldId, campaignId, sheet }),
  rpgAction: (worldId: string, campaignId: string, kind: string, target: string, item: string, qty: number, reason: string) => call<string>("rpg_action", { worldId, campaignId, input: { kind, target, item, qty, reason } }),
  rpgActionFull: (worldId: string, campaignId: string, input: { kind: string; target: string; item: string; qty: number; reason: string; item2: string; qty2: number }) => call<string>("rpg_action", { worldId, campaignId, input }),
  rpgSetRules: (worldId: string, campaignId: string, rules: RuleSet) => call<void>("rpg_set_rules", { worldId, campaignId, rules }),
  combatStart: (worldId: string, campaignId: string, enemies: EnemySpec[], allies: string[], reason: string) => call<CombatReport>("combat_start", { worldId, campaignId, enemies, allies, reason }),
  combatAct: (worldId: string, campaignId: string, action: CombatAction) => call<CombatReport>("combat_act", { worldId, campaignId, action }),
  combatAbort: (worldId: string, campaignId: string, reason: string) => call<CombatReport>("combat_abort", { worldId, campaignId, reason }),
  worldSimOverview: (worldId: string, campaignId: string, gmView: boolean) => call<WorldSimOverview>("world_sim_overview", { worldId, campaignId, gmView }),
  planTravel: (worldId: string, campaignId: string, to: string) => call<TravelPlan>("plan_travel", { worldId, campaignId, to }),
  travelTo: (worldId: string, campaignId: string, to: string) => call<TravelDone>("travel_to", { worldId, campaignId, to }),
  addMapMarker: (worldId: string, campaignId: string, place: string, label: string, note: string) => call<string>("add_map_marker", { worldId, campaignId, place, label, note }),
  removeMapMarker: (worldId: string, campaignId: string, id: string) => call<void>("remove_map_marker", { worldId, campaignId, id }),
  setWeatherToday: (worldId: string, campaignId: string, weather: string) => call<void>("set_weather_today", { worldId, campaignId, weather }),
  findPlaces: (worldId: string, query: string) => call<Node[]>("find_places", { worldId, query }),
  perfSnapshot: () => call<PerfSnapshot>("perf_snapshot"),
  setPerfMode: (mode: string) => call<void>("set_perf_mode", { mode }),
  pauseFrom: (priority: Priority | null) => call<void>("pause_from", { priority }),
  taskPause: (id: number, paused: boolean) => call<boolean>("task_pause", { id, paused }),
  taskCancel: (id: number) => call<boolean>("task_cancel", { id }),
  reportRender: (view: string, nodes: number, edges: number, ms: number) => call<void>("report_render", { view, nodes, edges, ms }).catch(() => {}),
  listManagedModels: () => call<ProcStatus[]>("list_managed_models"),
  saveManagedModel: (profile: ModelProfile) => call<void>("save_managed_model", { profile }),
  deleteManagedModel: (id: string) => call<void>("delete_managed_model", { id }),
  modelStart: (id: string, restart: boolean) => call<void>("model_start", { id, restart }),
  modelStop: (id: string) => call<void>("model_stop", { id }),
  estimateModel: (args: string[]) => call<MemEstimate | null>("estimate_model", { args }),
  /** 内存建议（只给建议，不改配置）。 */
  memoryAdvice: (profile: ModelProfile) => call<MemoryAdvice | null>("memory_advice", { profile }),
  dbProbe: (worldId: string) => call<DbProbe>("db_probe", { worldId }),
  appInfo: () => call<AppInfo>("app_info"),
  /** 检查更新：有新版本返回信息，已是最新返回 null。 */
  updateCheck: () => call<{ current: string; version: string; date: string | null; notes: string | null } | null>("update_check"),
  /** 下载、校验签名并安装，成功后程序会重启。 */
  updateInstall: () => call<void>("update_install"),
  /** 界面偏好（键以 ui. 开头），存在资料库里。 */
  uiPref: (key: string) => call<string | null>("ui_pref", { key }),
  setUiPref: (key: string, value: string) => call<void>("set_ui_pref", { key, value }),
  startupError: () => call<string | null>("startup_error"),
  listWorlds: () => call<WorldInfo[]>("list_worlds"),
  createWorld: (name: string) => call<WorldInfo>("create_world", { name }),
  worldOverview: (worldId: string) => call<WorldOverview>("world_overview", { worldId }),
  listCommits: (worldId: string, limit = 50) => call<CommitInfo[]>("list_commits", { worldId, limit }),
  undoCommit: (worldId: string, commitId: string) => call<string | null>("undo_commit", { worldId, commitId }),

  listScripts: (worldId: string) => call<Script[]>("list_scripts", { worldId }),
  getScript: (worldId: string, scriptId: string) => call<ScriptView>("get_script", { worldId, scriptId }),
  /** 剧本的玩法配置（保存的，或按题材推断的）。 */
  scriptGameProfile: (worldId: string, scriptId: string) => call<GameProfile>("script_game_profile", { worldId, scriptId }),
  checkScript: (worldId: string, scriptId: string) => call<ScriptFinding[]>("check_script", { worldId, scriptId }),
  /** 删除剧本：条目、角色、剧情图一起进回收站（删除前自动保存一个版本）。 */
  deleteScript: (worldId: string, scriptId: string) => call<number>("delete_script", { worldId, scriptId }),
  restoreScript: (worldId: string, scriptId: string) => call<number>("restore_script", { worldId, scriptId }),
  scriptTrash: (worldId: string) => call<DeletedScript[]>("script_trash", { worldId }),
  copyScript: (worldId: string, scriptId: string, title: string) => call<Script>("copy_script", { worldId, scriptId, title }),
  scriptVersions: (worldId: string, scriptId: string) => call<ScriptVersion[]>("script_versions", { worldId, scriptId }),
  createScriptVersion: (worldId: string, scriptId: string, name: string) => call<ScriptVersion>("create_script_version", { worldId, scriptId, name }),
  diffScriptVersion: (worldId: string, versionId: string) => call<VersionDiff>("diff_script_version", { worldId, versionId }),
  restoreScriptVersion: (worldId: string, versionId: string) => call<number>("restore_script_version", { worldId, versionId }),
  deleteScriptVersion: (worldId: string, versionId: string) => call<null>("delete_script_version", { worldId, versionId }),
  /** 打开示例世界（没有就创建；不会覆盖已有世界）。 */
  openDemoWorld: () => call<WorldInfo>("open_demo_world"),
  /** 运行监控：最近的请求与统计。 */
  runMonitor: (hours: number, recent: number) => call<[RecentRequest[], RunStats[]]>("run_monitor", { hours, recent }),
  memorySettings: (worldId: string, campaignId: string) => call<MemorySettings>("memory_settings", { worldId, campaignId }),
  setMemorySettings: (worldId: string, campaignId: string, settings: MemorySettings) => call<null>("set_memory_settings", { worldId, campaignId, settings }),
  /** 立即压缩上下文（replace：重新生成这份自动摘要）。 */
  compressMemory: (worldId: string, campaignId: string, replace: string | null) => call<SessionSummary>("compress_memory", { worldId, campaignId, replace }),
  deleteSummary: (worldId: string, campaignId: string, summaryId: string) => call<null>("delete_summary", { worldId, campaignId, summaryId }),
  /** 剧透模式（全局，默认关闭）。 */
  spoilerMode: () => call<boolean>("spoiler_mode"),
  setSpoilerMode: (on: boolean) => call<null>("set_spoiler_mode", { on }),
  searchSettings: () => call<{ config: SearchConfig; has_key: string[] }>("search_settings"),
  /** keys：要保存的 Key（空字符串表示删除）。 */
  saveSearchSettings: (config: SearchConfig, keys: Record<string, string>) => call<null>("save_search_settings", { config, keys }),
  /** 手动联网搜索（结果只显示）。 */
  webSearch: (query: string) => call<SearchResult[]>("web_search", { query }),
  dataDirInfo: () => call<{ current: string; default: string; configured: string | null; warning: string | null; env_override: boolean }>("data_dir_info"),
  /** 迁移数据目录（复制 → 校验 → 重启后切换）。 */
  migrateDataDir: (target: string) => call<{ files: number; bytes: number; databases: number; target: string }>("migrate_data_dir", { target }),
  resetDataDir: () => call<null>("reset_data_dir"),
  /** 多条件筛选节点。 */
  filterNodes: (worldId: string, filter: NodeFilter) => call<Node[]>("filter_nodes", { worldId, filter }),
  payloadFields: (worldId: string, kind: NodeKind | null) => call<FieldInfo[]>("payload_fields", { worldId, kind }),
  trashCampaign: (worldId: string, campaignId: string) => call<number>("trash_campaign", { worldId, campaignId }),
  restoreCampaign: (worldId: string, campaignId: string) => call<number>("restore_campaign", { worldId, campaignId }),
  campaignTrash: (worldId: string) => call<DeletedCampaign[]>("campaign_trash", { worldId }),
  /** 永久删除回收站里的战役（先自动备份世界）。 */
  purgeCampaign: (worldId: string, campaignId: string) => call<number>("purge_campaign", { worldId, campaignId }),
  deleteNpcMemory: (worldId: string, campaignId: string, memoryId: string) => call<null>("delete_npc_memory", { worldId, campaignId, memoryId }),
  forgetFact: (worldId: string, campaignId: string, seq: number) => call<null>("forget_fact", { worldId, campaignId, seq }),
  deleteBackup: (worldId: string, fileName: string) => call<null>("delete_backup", { worldId, fileName }),
  trashWorld: (worldId: string) => call<null>("trash_world", { worldId }),
  worldTrash: () => call<WorldInfo[]>("world_trash"),
  restoreWorld: (worldId: string) => call<WorldInfo>("restore_world", { worldId }),
  npcMemoryList: (worldId: string, campaignId: string, npcId: string) => call<NpcMemory[]>("npc_memory_list", { worldId, campaignId, npcId }),
  /** 存档（name 为空 = 快速存档）。 */
  savePoint: (worldId: string, campaignId: string, name: string) => call<Checkpoint>("save_point", { worldId, campaignId, name }),
  /** 快速读档：从最近的快速存档开出新时间线。 */
  quickLoad: (worldId: string, campaignId: string) => call<Campaign>("quick_load", { worldId, campaignId }),
  autoMediaSettings: () => call<{ scene_images: boolean; portraits: boolean; pre_voice: boolean }>("auto_media_settings"),
  setAutoMediaSettings: (settings: { scene_images: boolean; portraits: boolean; pre_voice: boolean }) => call<null>("set_auto_media_settings", { settings }),
  transcriptionAvailable: () => call<boolean>("transcription_available"),
  /** 语音输入：录音（base64）转文字。 */
  transcribeAudio: (audioBase64: string, mime: string) => call<string>("transcribe_audio", { audioBase64, mime }),
  /** 剧本条目修改后，可能需要复查的世界树节点。 */
  scriptItemRelatedNodes: (worldId: string, itemId: string) => call<Node[]>("script_item_related_nodes", { worldId, itemId }),
  /** 全局搜索。 */
  globalSearch: (worldId: string, query: string) => call<GlobalHit[]>("global_search", { worldId, query }),
  /** 待确认的图片建议。 */
  pendingImages: (worldId: string) => call<{ node_id: string; title: string; kind: string; prompt: string; created_at: number }[]>("pending_images", { worldId }),
  dismissPendingImage: (worldId: string, nodeId: string) => call<null>("dismiss_pending_image", { worldId, nodeId }),
  scriptPlayerCharacters: (worldId: string, scriptId: string) => call<PlayerCharacter[]>("script_player_characters", { worldId, scriptId }),
  /** AI 按剧本生成新的玩家角色（会调用模型，不写入剧本）。 */
  generatePc: (worldId: string, scriptId: string, hint: string) => call<PlayerCharacter>("generate_pc", { worldId, scriptId, hint }),
  /** AI 生成任务（会调用模型）：只返回提案与冲突检查。 */
  generateQuest: (worldId: string, campaignId: string, mode: "situation" | "worldview", hint: string) => call<QuestProposal>("generate_quest", { worldId, campaignId, mode, hint }),
  questAdd: (worldId: string, campaignId: string, quest: NewQuest) => call<string>("quest_add", { worldId, campaignId, quest }),
  campaignPc: (worldId: string, campaignId: string) => call<PlayerCharacter | null>("campaign_pc", { worldId, campaignId }),
  campaignSetPc: (worldId: string, campaignId: string, pc: PlayerCharacter) => call<null>("campaign_set_pc", { worldId, campaignId, pc }),
  /** AI 逻辑检查（会调用模型）。 */
  reviewScript: (worldId: string, scriptId: string) => call<ScriptFinding[]>("review_script", { worldId, scriptId }),
  createScript: (worldId: string, script: NewScript) => call<Script>("create_script", { worldId, script }),
  updateScript: (worldId: string, id: string, patch: ScriptPatch, rev: number) =>
    call<Script>("update_script", { worldId, id, patch, rev }),
  createScriptItem: (worldId: string, item: NewScriptItem) => call<ScriptItem>("create_script_item", { worldId, item }),
  updateScriptItem: (worldId: string, id: string, patch: ScriptItemPatch, rev: number) =>
    call<ScriptItem>("update_script_item", { worldId, id, patch, rev }),
  deleteScriptItem: (worldId: string, id: string, rev: number) => call<number>("delete_script_item", { worldId, id, rev }),
  searchScriptItems: (worldId: string, scriptId: string | null, query: string) =>
    call<ScriptItem[]>("search_script_items", { worldId, scriptId, query }),
  getNode: (worldId: string, id: string) => call<Node | null>("get_node", { worldId, id }),

  treeChildren: (worldId: string, parentId: string | null, offset = 0, limit = 500) =>
    call<TreeNode[]>("tree_children", { worldId, parentId, offset, limit }),
  nodeDetail: (worldId: string, id: string) => call<NodeDetail>("node_detail", { worldId, id }),
  createNode: (worldId: string, node: NewNode) => call<Node>("create_node", { worldId, node }),
  updateNode: (worldId: string, id: string, patch: NodePatch, rev: number) => call<Node>("update_node", { worldId, id, patch, rev }),
  moveNode: (worldId: string, id: string, newParent: string | null, position: Position, rev: number) =>
    call<Node>("move_node", { worldId, id, newParent, position, rev }),
  deleteNode: (worldId: string, id: string, rev: number) => call<number>("delete_node", { worldId, id, rev }),
  restoreNode: (worldId: string, id: string) => call<number>("restore_node", { worldId, id }),
  listTrash: (worldId: string) => call<Node[]>("list_trash", { worldId }),
  createRelation: (worldId: string, relation: NewRelation) => call<Relation>("create_relation", { worldId, relation }),
  updateRelation: (worldId: string, id: string, patch: RelationPatch, rev: number) =>
    call<Relation>("update_relation", { worldId, id, patch, rev }),
  deleteRelation: (worldId: string, id: string, rev: number) => call<void>("delete_relation", { worldId, id, rev }),
  relationTypes: () => call<RelationType[]>("relation_types"),
  addCastMember: (worldId: string, scriptId: string, nodeId: string, role: string) => call<unknown>("add_cast_member", { worldId, scriptId, nodeId, role }),
  relationNetwork: (worldId: string, query: NetworkQuery) => call<Network>("relation_network", { worldId, query }),
  bondHistory: (worldId: string, campaignId: string, a: string, b: string) => call<BondChange[]>("bond_history", { worldId, campaignId, a, b }),
  personBonds: (worldId: string, campaignId: string, id: string) => call<BondSummary[]>("person_bonds", { worldId, campaignId, id }),
  relationsAmong: (worldId: string, ids: string[]) => call<Relation[]>("relations_among", { worldId, ids }),
  searchNodes: (worldId: string, query: string, options: SearchOptions) => call<SearchHit[]>("search_nodes", { worldId, query, options }),
  nodeGraph: (worldId: string, center: string, depth: number, limit: number) => call<Graph>("node_graph", { worldId, center, depth, limit }),
  checkConsistency: (worldId: string) => call<Finding[]>("check_consistency", { worldId }),
  kindCounts: (worldId: string) => call<[string, number][]>("kind_counts", { worldId }),
  listVersions: (worldId: string) => call<WorldVersion[]>("list_versions", { worldId }),
  createVersion: (worldId: string, name: string, note: string) => call<WorldVersion>("create_version", { worldId, name, note }),
  restoreVersion: (worldId: string, versionId: string) => call<string>("restore_version", { worldId, versionId }),
  listBackups: (worldId: string) => call<BackupInfo[]>("list_backups", { worldId }),
  restoreBackup: (worldId: string, fileName: string) => call<string>("restore_backup", { worldId, fileName }),
  backupNow: (worldId: string) => call<BackupInfo | null>("backup_now", { worldId }),
  checkIntegrity: (worldId: string) => call<IntegrityReport>("check_integrity", { worldId }),
  repairIntegrity: (worldId: string) => call<RepairReport>("repair_integrity", { worldId }),
  exportWorld: (worldId: string, path: string, includeDeleted: boolean) => call<BundleManifest>("export_world", { worldId, path, includeDeleted }),
  importIntoWorld: (worldId: string, path: string, strategy: ConflictStrategy) => call<ImportReport>("import_into_world", { worldId, path, strategy }),
  importAsNewWorld: (path: string, name: string | null) => call<WorldInfo>("import_as_new_world", { path, name }),

  plotGraph: (worldId: string, scriptId: string) => call<PlotGraph>("plot_graph", { worldId, scriptId }),
  createPlotNode: (worldId: string, node: NewPlotNode) => call<PlotNode>("create_plot_node", { worldId, node }),
  updatePlotNode: (worldId: string, id: string, patch: PlotNodePatch, rev: number) => call<PlotNode>("update_plot_node", { worldId, id, patch, rev }),
  deletePlotNode: (worldId: string, id: string, rev: number) => call<void>("delete_plot_node", { worldId, id, rev }),
  createPlotEdge: (worldId: string, edge: NewPlotEdge) => call<PlotEdge>("create_plot_edge", { worldId, edge }),
  updatePlotEdge: (worldId: string, id: string, edge: EdgeUpdate, rev: number) => call<PlotEdge>("update_plot_edge", { worldId, id, edge, rev }),
  deletePlotEdge: (worldId: string, id: string, rev: number) => call<void>("delete_plot_edge", { worldId, id, rev }),
  analyzePlot: (worldId: string, scriptId: string) => call<PlotReport>("analyze_plot", { worldId, scriptId }),
  simStart: (worldId: string, scriptId: string) => call<SimState>("sim_start", { worldId, scriptId }),
  simOptions: (worldId: string, sim: SimState) => call<EdgeOption[]>("sim_options", { worldId, sim }),
  simStep: (worldId: string, sim: SimState, edgeId: string, mode: CheckMode, seed: number) => call<SimStep>("sim_step", { worldId, sim, edgeId, mode, seed }),
  listCampaigns: (worldId: string) => call<Campaign[]>("list_campaigns", { worldId }),
  createCampaign: (worldId: string, scriptId: string | null, name: string) => call<Campaign>("create_campaign", { worldId, scriptId, name }),
  campaignStatus: (worldId: string, campaignId: string) => call<CampaignStatus>("campaign_status", { worldId, campaignId }),
  dismissIncompleteTurn: (worldId: string, campaignId: string) => call<void>("dismiss_incomplete_turn", { worldId, campaignId }),
  endCampaign: (worldId: string, campaignId: string, ending: string) => call<Checkpoint>("end_campaign", { worldId, campaignId, ending }),
  timelineForkPoints: (worldId: string, campaignId: string) => call<ForkPoint[]>("timeline_fork_points", { worldId, campaignId }),
  timelineFork: (worldId: string, campaignId: string, checkpointId: number, name: string, reseed: boolean) =>
    call<Campaign>("timeline_fork", { worldId, campaignId, checkpointId, name, reseed }),
  timelineVerify: (worldId: string, campaignId: string) => call<VerifyReport>("timeline_verify", { worldId, campaignId }),
  timelineRename: (worldId: string, campaignId: string, name: string) => call<void>("timeline_rename", { worldId, campaignId, name }),
  timelineArchive: (worldId: string, campaignId: string, archived: boolean) => call<void>("timeline_archive", { worldId, campaignId, archived }),
  timelineCompare: (worldId: string, campaignIds: string[]) => call<TimelineComparison>("timeline_compare", { worldId, campaignIds }),
  archiveList: (worldId: string) => call<ArchiveEntry[]>("archive_list", { worldId }),
  archiveStats: (worldId: string, campaignId: string) => call<CampaignStats>("archive_stats", { worldId, campaignId }),
  archiveEndings: (worldId: string, scriptId: string) => call<EndingDistribution>("archive_endings", { worldId, scriptId }),
  archiveChapters: (worldId: string, campaignId: string) => call<Chapter[]>("archive_chapters", { worldId, campaignId }),
  archiveMarkChapter: (worldId: string, campaignId: string, title: string) => call<number>("archive_mark_chapter", { worldId, campaignId, title }),
  archiveRenameChapter: (worldId: string, campaignId: string, seq: number, title: string) => call<void>("archive_rename_chapter", { worldId, campaignId, seq, title }),
  archiveRemoveChapter: (worldId: string, campaignId: string, seq: number) => call<void>("archive_remove_chapter", { worldId, campaignId, seq }),
  archiveChronicle: (worldId: string, campaignId: string) => call<Chronicle>("archive_chronicle", { worldId, campaignId }),
  archiveSaveChronicle: (worldId: string, campaignId: string, path: string) => call<void>("archive_save_chronicle", { worldId, campaignId, path }),
  archiveSearch: (worldId: string, filter: SearchFilter) => call<EventHit[]>("archive_search", { worldId, filter }),
  archiveExport: (worldId: string, campaignId: string, path: string, withDescendants: boolean) => call<number>("archive_export", { worldId, campaignId, path, withDescendants }),
  archiveImport: (worldId: string, path: string) => call<ImportResult>("archive_import", { worldId, path }),
  archiveCheck: (worldId: string) => call<CampaignCheck[]>("archive_check", { worldId }),
  campaignTakeEdge: (worldId: string, campaignId: string, edgeId: string) => call<TransitionOutcome>("campaign_take_edge", { worldId, campaignId, edgeId }),

  gmTurn: (worldId: string, campaignId: string, input: string) => call<TurnResult>("gm_turn", { worldId, campaignId, input }),
  cancelGmTurn: (campaignId: string) => call<boolean>("cancel_gm_turn", { campaignId }),
  campaignLog: (worldId: string, campaignId: string, afterSeq = 0) => call<EventEntry[]>("campaign_log", { worldId, campaignId, afterSeq }),
  campaignMemory: (worldId: string, campaignId: string) => call<CampaignMemory>("campaign_memory", { worldId, campaignId }),
  gmSummarize: (worldId: string, campaignId: string) => call<SessionSummary>("gm_summarize", { worldId, campaignId }),
  confirmSummary: (worldId: string, summaryId: string, text: string | null) => call<void>("confirm_summary", { worldId, summaryId, text }),

  listChangesets: (worldId: string, status: ChangesetStatus | null) =>
    call<Changeset[]>("list_changesets", { worldId, status }),
  applyChangeset: (worldId: string, id: string, accepted: number[] | null) =>
    call<Changeset>("apply_changeset", { worldId, id, accepted }),
  rejectChangeset: (worldId: string, id: string) => call<void>("reject_changeset", { worldId, id }),
  reviseOp: (worldId: string, id: string, index: number, op: Op) =>
    call<Changeset>("revise_changeset_op", { worldId, id, index, op }),

  /** 开始生成；`images` 是参考图片（data URL 或 `node-image:<ID>`），需要模型服务通过了看图测试。 */
  startGeneration: (worldId: string, task: Task, images: string[] = []) => call<string>("start_generation", { worldId, task, images }),
  nodeImages: (worldId: string, nodeId: string) => call<NodeImage[]>("node_images", { worldId, nodeId }),
  nodeImageAdd: (worldId: string, nodeId: string, data: string, width: number, height: number, caption: string) => call<NodeImage>("node_image_add", { worldId, nodeId, data, width, height, caption }),
  nodeImageData: (worldId: string, imageId: string) => call<string>("node_image_data", { worldId, imageId }),
  nodeImageCaption: (worldId: string, imageId: string, caption: string) => call<void>("node_image_caption", { worldId, imageId, caption }),
  nodeImageCover: (worldId: string, imageId: string) => call<void>("node_image_cover", { worldId, imageId }),
  nodeImageDelete: (worldId: string, imageId: string) => call<void>("node_image_delete", { worldId, imageId }),
  /** 封面缩略图（没有图片的节点不在结果里；thumb 为空表示还没生成）。 */
  nodeCoverThumbs: (worldId: string, nodeIds: string[]) => call<ImageThumb[]>("node_cover_thumbs", { worldId, nodeIds }),
  nodeImageThumbs: (worldId: string, nodeId: string) => call<ImageThumb[]>("node_image_thumbs", { worldId, nodeId }),
  nodeImageSetThumb: (worldId: string, imageId: string, data: string) => call<void>("node_image_set_thumb", { worldId, imageId, data }),
  illustratedNodes: (worldId: string) => call<IllustratedNode[]>("illustrated_nodes", { worldId }),
  sceneView: (worldId: string, campaignId: string) => call<SceneView>("scene_view", { worldId, campaignId }),
  describeNodeImage: (worldId: string, imageId: string) => call<string>("describe_node_image", { worldId, imageId }),
  retryJob: (worldId: string, jobId: string) => call<string>("retry_job", { worldId, jobId }),
  fillDeadEnds: (worldId: string, scriptId: string) => call<string[]>("fill_dead_ends", { worldId, scriptId }),
  cancelJob: (jobId: string) => call<boolean>("cancel_job", { jobId }),
  listJobs: (worldId: string) => call<GenJob[]>("list_jobs", { worldId }),

  discoverServices: (extraUrls: string[]) => call<DiscoveredService[]>("discover_services", { extraUrls }),
  scanModelFolder: (folder: string) => call<ScanResult>("scan_model_folder", { folder }),
  listProviders: () => call<ProviderView[]>("list_providers"),
  /** 读取服务的模型列表（不计费）；apiKey 为空时用已保存的 Key。 */
  providerModels: (id: string | null, baseUrl: string, apiKey: string | null) => call<string[]>("provider_models", { id, baseUrl, apiKey }),
  providerVoices: (id: string | null, baseUrl: string, apiKey: string | null) => call<string[]>("provider_voices", { id, baseUrl, apiKey }),
  /** 用生图服务生成一张图片（不保存）。 */
  generateImage: (prompt: string, size: string | null) => call<{ data_url: string; revised_prompt: string | null; provider: string; ms: number }>("generate_image", { prompt, size }),
  saveProvider: (config: ProviderConfig, apiKey: string | null) =>
    call<ProviderConfig>("save_provider", { input: { config, api_key: apiKey } }),
  deleteProvider: (id: string) => call<void>("delete_provider", { id }),
  testProvider: (id: string, contextHint: number | null) => call<Capabilities>("test_provider", { id, contextHint }),
  listRoutes: () => call<[string, string][]>("list_routes"),
  setRoute: (task: string, providerId: string | null) => call<void>("set_route", { task, providerId }),
  providerStats: () => call<ProviderStats[]>("provider_stats"),
};

export function formatTime(ms: number | null | undefined): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
