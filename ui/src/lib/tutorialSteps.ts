// 游戏教程的步骤：GM 玻璃球说什么、指向哪里（data-tour 锚点）、在哪个页面、什么时候算完成。
// 完成条件读的都是真实数据（模型配置、战役、事件日志），等你真的做完那一步才继续。
import { api } from "../api";
import type { Page } from "../App";

export type TourEnv = { worldId: string | null };

export type TourStep = {
  id: string;
  /** GM 说的话。 */
  say: string;
  /** 要高亮的元素（data-tour 的值）；可以给几个，用第一个找得到的。 */
  target?: string[];
  /** 进入这一步时切到的页面。 */
  page?: Page["name"];
  /** 气泡里的动作按钮。 */
  action?: { kind: "demo"; label: string };
  /** 自动完成条件（轮询）。没有就显示「下一步」。 */
  until?: (env: TourEnv) => Promise<boolean>;
  /** 等待时的提示。 */
  waiting?: string;
  /** 允许跳过这一步（例如暂时没有模型）。 */
  skipLabel?: string;
};

async function latestCampaign(worldId: string | null) {
  if (!worldId) return null;
  const cs = await api.listCampaigns(worldId);
  return [...cs].reverse().find((c) => !c.archived && c.script_id) ?? null;
}

async function log(worldId: string | null) {
  const c = await latestCampaign(worldId);
  if (!c || !worldId) return [];
  return api.campaignLog(worldId, c.id);
}

export const TOUR: TourStep[] = [
  {
    id: "hello",
    say: "你好，我是这里的 GM。接下来我带你玩一小段：配置模型、打开示例世界、选角色、听开场、做几个行动。随时可以暂停，下次从这里接着来。",
  },
  {
    id: "models",
    page: "settings",
    target: ["models-scan"],
    say: "先让我能说话：在你的电脑上启动模型服务（llama-server、Ollama、LM Studio 都可以），然后点「扫描」，把找到的服务添加进来并测试。也可以在下面添加云端服务。",
    until: async () => (await api.listProviders()).some((p) => p.config.server_kind !== "breeze_tts" && p.config.capabilities?.chat),
    waiting: "配置好、测试通过后会自动继续",
    skipLabel: "暂时没有模型，先往下看",
  },
  {
    id: "demo",
    say: "我准备了一个示例世界「雾湾」：一座熄灭的灯塔、一个守塔老人和两个可选角色。不需要等 AI 生成，打开就能玩。",
    action: { kind: "demo", label: "打开示例世界" },
  },
  {
    id: "campaign",
    page: "game",
    target: ["new-script", "new-campaign"],
    say: "这里是游戏页。在右上角选剧本「灯塔守夜」，然后点「新建战役」。一个战役就是一局游戏，存档会一直记着。",
    until: async (env) => !!(await latestCampaign(env.worldId)),
    waiting: "选好剧本，点「新建战役」",
  },
  {
    id: "pick",
    page: "game",
    target: ["pc-picker"],
    say: "选一个你想扮演的角色，看看他的身世。也可以让 AI 按剧本再生成一个新角色。选好后点下面的按钮开始。",
    until: async (env) => {
      const c = await latestCampaign(env.worldId);
      if (!c || !env.worldId) return false;
      return !!(await api.campaignPc(env.worldId, c.id)) || (await log(env.worldId)).some((e) => e.kind === "narration");
    },
    waiting: "选好角色后自动继续",
  },
  {
    id: "opening",
    page: "game",
    target: ["gm-stage", "opening"],
    say: "现在由我讲述开场。我构思的时候玻璃球里会转一圈细弧，讲述时会展开；如果配置了语音，还会朗读。",
    until: async (env) => (await log(env.worldId)).some((e) => e.kind === "narration"),
    waiting: "GM 正在构思开场（本地模型可能要一两分钟）",
    skipLabel: "跳过（没有模型时 GM 不能讲述）",
  },
  {
    id: "orb",
    page: "game",
    target: ["gm-stage"],
    say: "我说的话都留在这颗玻璃球里：讲完会停在球里，你行动时自动收起。想再看就点一下玻璃球，用 ◀ ▶（或键盘左右键）翻看之前我说过的话；剧情里的「GM · 第 N 回合」小条也能点开对应那一段。",
  },
  {
    id: "choices",
    page: "game",
    target: ["choices", "input"],
    say: "每回合我都会给你几个可以做的事，点一下就是你的行动。你也完全可以不选，自己写。先点一个试试。",
    until: async (env) => (await log(env.worldId)).some((e) => e.kind === "player"),
    waiting: "点一个选项，或者自己输入",
    skipLabel: "先跳过",
  },
  {
    id: "free",
    page: "game",
    target: ["input"],
    say: "这次自己写：比如「我问镇长灯塔是怎么熄的」或者「我爬上码头的灯柱看看海面」。需要掷骰时，玻璃球会展开，显示骰子、点数和判定结果——骰子由程序掷，我只能按结果讲。",
    until: async (env) => (await log(env.worldId)).filter((e) => e.kind === "player").length >= 2,
    waiting: "输入一个行动，按 Enter 发送",
    skipLabel: "先跳过",
  },
  {
    id: "panels",
    page: "game",
    target: ["tabs"],
    say: "右边是游戏面板：「任务」里看目标（可以切换显示方式，也能让我生成新任务），「角色」里是属性、背包和难度，「状态」里是这一回合的检定和变化。每回合有新变化时，剧情区下方会弹出小卡片，可以固定或关掉。",
  },
  {
    id: "saves",
    page: "game",
    target: ["tab-timeline"],
    say: "「时间线」里可以存档、快速存档和读档。读档会从存档点开出一条新时间线，现在这条不会被覆盖，所以放心试不同的选择。",
  },
  {
    id: "spoiler",
    page: "game",
    target: ["spoiler"],
    say: "这是剧透模式：打开后能看到秘密、真相和 AI 的意图，适合你自己当主持人的时候。自己当玩家就别开，免得剧透。",
  },
  {
    id: "search",
    target: ["global-search"],
    say: "忘了某个人物、地点或者之前发生过什么？按 Ctrl+K（或点这里）全局搜索：世界树、剧本、战役、任务和游戏记录都能搜到。",
  },
  {
    id: "create",
    target: ["nav-tree", "nav-script"],
    say: "想自己写故事的时候：先在「剧本」里生成并检查剧本，再根据剧本生成「世界树」；AI 的每处修改都会先进「审阅与任务」，你确认后才写入。教程到这里就结束了，祝你玩得开心！",
  },
];
