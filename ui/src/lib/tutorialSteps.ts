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
  /** 进入这一步时切到的页面（lobby = 大厅；game = 游戏空间）。 */
  page?: Page["name"] | "lobby";
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

/** 能让 GM 说话：内置模型已经在运行，或者配置了能对话的模型服务（本地或云端）。 */
async function hasModel() {
  const v = await api.llmServeStatus().catch(() => null);
  if (v?.process?.pid && v.stats.healthy) return true;
  return (await api.listProviders()).some((p) => p.config.service === "chat" && p.config.server_kind !== "breeze_tts" && p.config.capabilities?.chat);
}

async function log(worldId: string | null) {
  const c = await latestCampaign(worldId);
  if (!c || !worldId) return [];
  return api.campaignLog(worldId, c.id);
}

export const TOUR: TourStep[] = [
  {
    id: "hello",
    say: "你好，我是这里的 GM。接下来我带你玩一小段：准备模型、打开示例世界、选角色、听开场、做几个行动，最后看看面板、存档和联机。随时可以暂停，之后从标题栏世界菜单的「游戏教程」接着来。",
  },
  {
    id: "models",
    page: "local",
    target: ["local-serve"],
    say: "先让我能说话。软件自带 llama.cpp：在「本地模型」里下载默认模型，或者在「模型库」添加电脑里已有的 GGUF 文件，然后点「启动」。显卡层数、上下文这些参数会按你的电脑自动调好。用云端 API 的话到「云端模型」里填地址和密钥；已经在跑 Ollama、LM Studio 的，可以在「模型服务」里扫描添加。",
    until: hasModel,
    waiting: "模型启动好（或配置好模型服务）后会自动继续",
    skipLabel: "暂时没有模型，先往下看",
  },
  {
    id: "demo",
    say: "我准备了一个示例世界「雾湾」：一座熄灭的灯塔、一个守塔老人和两个可选角色。不需要等 AI 生成，打开就能玩。",
    action: { kind: "demo", label: "打开示例世界" },
  },
  {
    id: "campaign",
    page: "lobby",
    target: ["lobby-scripts", "lobby-create"],
    say: "这里是大厅。在剧本库里选「灯塔守夜」，再点「下一步：选角色」。一局游戏就是一个战役，进度每回合自动保存，下次在大厅点「继续」就能回来。",
    until: async (env) => !!(await latestCampaign(env.worldId)),
    waiting: "选好剧本，点「下一步：选角色」",
  },
  {
    id: "pick",
    target: ["pc-picker"],
    say: "选一个你想扮演的角色，看看他的身世和随身物品。也可以让 AI 按剧本再生成一个新角色。选好后点下面的按钮。",
    until: async (env) => {
      const c = await latestCampaign(env.worldId);
      if (!c || !env.worldId) return false;
      return !!(await api.campaignPc(env.worldId, c.id)) || (await log(env.worldId)).some((e) => e.kind === "narration");
    },
    waiting: "选好角色后自动继续",
  },
  {
    id: "opening",
    target: ["lobby-start", "gm-stage"],
    say: "确认难度和掷骰方式后，点「确认，初始化世界」。准备好后会自动进入游戏，由我讲述开场：构思时玻璃球里会转一圈细弧，讲述时字会一个个打出来；配置了语音还会朗读。",
    until: async (env) => (await log(env.worldId)).some((e) => e.kind === "narration"),
    waiting: "GM 正在构思开场（本地模型可能要一两分钟）",
    skipLabel: "跳过（没有模型时 GM 不能讲述）",
  },
  {
    id: "orb",
    page: "game",
    target: ["gm-stage"],
    say: "游戏里有两个框：左边的大框是你和 NPC 的对话，右边带玻璃球的小框是我的——我的叙述都记在这里，往上翻能看到之前的。两个框中间的分隔条可以拖动，调成你喜欢的宽度。",
  },
  {
    id: "choices",
    page: "game",
    target: ["choices", "input"],
    say: "每回合我都会给你几个可以做的事，点一下就是你的行动，会马上出现在对话里。你也完全可以不选，自己写。先点一个试试。",
    until: async (env) => (await log(env.worldId)).some((e) => e.kind === "player"),
    waiting: "点一个选项，或者自己输入",
    skipLabel: "先跳过",
  },
  {
    id: "free",
    page: "game",
    target: ["input"],
    say: "这次自己写：比如「我问守塔老人灯塔是怎么熄的」或者「我爬上码头的灯柱看看海面」。需要检定时会掷 3D 骰子，显示点数和成败——骰子由程序掷，我只能按结果讲，不能替你改。",
    until: async (env) => (await log(env.worldId)).filter((e) => e.kind === "player").length >= 2,
    waiting: "输入一个行动，按 Enter 发送",
    skipLabel: "先跳过",
  },
  {
    id: "hands-on",
    page: "game",
    target: ["scene-bar"],
    say: "输入框上面这一排是这里能亲手互动的东西：点一扇门可以开锁或撬锁，点箱子、桌子可以仔细调查，还有物品、组合、潜行、医疗。需要你亲自动手时我也会直接递给你一张操作卡片——成败由程序按规则结算。不想用这些也没关系，直接写你自己的办法就行。",
  },
  {
    id: "panels",
    page: "game",
    target: ["panel-fab"],
    say: "右下角的「面板」按钮会展开右侧图标栏：「任务」看目标（也能让我生成新任务），「角色」是属性、背包和难度，「状态」是这一回合的检定和变化，还有人物和人物关系。每回合有新变化时，对话下方会弹出小卡片，可以固定或关掉。",
  },
  {
    id: "saves",
    page: "game",
    target: ["tab-timeline"],
    say: "「时间线」里可以存档、快速存档和读档。读档会从存档点开出一条新时间线，现在这条不会被覆盖，所以放心试不同的选择。",
  },
  {
    id: "settings",
    page: "game",
    target: ["spoiler", "game-settings"],
    say: "图标栏最下面的齿轮是这局的设置：自动朗读、掷骰方式（程序掷、你点击掷，或者掷实体骰子再输入点数）、省电模式。这里的剧透模式打开后能看到秘密、真相和 AI 的意图，适合你自己当主持人；自己当玩家就别开。",
  },
  {
    id: "search",
    target: ["global-search"],
    say: "忘了某个人物、地点或者之前发生过什么？按 Ctrl+K（或点这里）全局搜索：世界树、剧本、战役、任务和游戏记录都能搜到。",
  },
  {
    id: "multiplayer",
    page: "lobby",
    target: ["lobby-mp"],
    say: "想和朋友一起玩？大厅里的「联机游戏」可以开房间或者加入别人的房间：每人扮演自己的角色，我按顺序处理大家的行动，战斗按先攻轮流；你悄悄做的事只有你和我知道。",
  },
  {
    id: "create",
    target: ["nav-tree", "nav-script"],
    say: "想自己写故事的时候，进入创作模式：先在「剧本」里生成并检查剧本，再根据剧本生成「世界树」；AI 的每处修改都会先进「审阅与任务」，你确认后才写入。教程到这里就结束了，祝你玩得开心！",
  },
];
