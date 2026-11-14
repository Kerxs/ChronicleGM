//! 任务执行：准备（读世界）→ 调用模型（不持有数据库）→ 转换并提交待审变更集。
//!
//! 三步分开，界面层可以在调用模型期间释放数据库锁。任何一步失败只把任务标记为失败，
//! 原始输出保留在任务记录中，世界内容不受影响，可以重试。

use chronicle_core::changeset::{self, Changeset};
use chronicle_core::jobs::{self, GenJob, JobStatus};
use chronicle_core::World;
use chronicle_llm::{
    CancelToken, Capabilities, ChatMessage, ChatRequest, ChatResponse, LlmClient, LlmError, OutputFormat,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::parse::extract_json;
use crate::tasks::{Task, build_prompt, convert};
use crate::templates::Templates;
use crate::{GenError, Result};

#[derive(Debug, Clone)]
pub struct GenOptions {
    /// 上下文预算（token）。
    pub context_budget: usize,
    pub temperature: f32,
    pub max_tokens: u32,
    pub stream: bool,
    /// 输出无法解析时，请模型修正的次数。
    pub repair_attempts: u32,
    /// 模型服务实际开启的上下文长度；已知时按"上下文 − 提示词 − 余量"动态设置输出上限。
    pub context_length: Option<usize>,
    /// `Some(false)`：请求关闭思考（来自模型服务的"思考模式"设置）。
    pub thinking: Option<bool>,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self { context_budget: 6000, temperature: 0.7, max_tokens: 8192, stream: true, repair_attempts: 1, context_length: None, thinking: None }
    }
}

impl GenOptions {
    /// 根据实测的上下文长度调整预算：最多用一半给参考资料，并留出输出空间。
    pub fn for_caps(caps: Option<&Capabilities>) -> Self {
        let mut o = Self::default();
        if let Some(c) = caps {
            if let Some(ctx) = c.context_length {
                let ctx = ctx as usize;
                // 参考资料最多占三分之一，其余留给输出（思考型模型的推理过程也计入输出）。
                o.context_budget = (ctx / 3).clamp(1500, 12000);
                o.max_tokens = 24000;
                o.context_length = Some(ctx);
            }
            o.stream = c.streaming;
        }
        o
    }
}

/// 一次模型请求的元数据（写入请求统计）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequestLog {
    pub ok: bool,
    pub attempts: u32,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

impl RequestLog {
    fn ok(r: &ChatResponse) -> Self {
        Self {
            ok: true,
            attempts: r.attempts,
            prompt_tokens: r.usage.prompt_tokens,
            completion_tokens: r.usage.completion_tokens,
            latency_ms: Some(r.latency_ms),
            error: None,
        }
    }
    fn err(e: &LlmError) -> Self {
        Self { ok: false, attempts: 1, error: Some(e.to_string()), ..Default::default() }
    }
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub job_id: String,
    pub task: Task,
    pub messages: Vec<ChatMessage>,
    pub schema_name: String,
    pub schema: Value,
    pub use_json_schema: bool,
    pub context_dropped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModelOutput {
    pub raw: String,
    pub value: Value,
    pub repaired: bool,
    /// 首次输出因长度上限被截断后，修正请求关闭了思考。
    pub thinking_disabled_on_retry: bool,
}

/// 新建任务并准备提示词。
pub fn prepare(
    world: &mut World,
    task: Task,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    provider: (&str, &str),
) -> Result<Prepared> {
    prepare_with_images(world, task, templates, caps, opts, provider, vec![])
}

/// 参考图片最多张数。
pub const MAX_REFERENCE_IMAGES: usize = 6;

/// 同 [`prepare`]，附带参考图片：`data:image/...` URL，或 `node-image:<图片 ID>`（世界树节点上已有的图片）。
/// 图片随任务参数保存，重试时使用同一组图片。需要模型服务通过了"看图"测试。
pub fn prepare_with_images(
    world: &mut World,
    task: Task,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    provider: (&str, &str),
    images: Vec<String>,
) -> Result<Prepared> {
    if images.len() > MAX_REFERENCE_IMAGES {
        return Err(GenError::Input(format!("参考图片最多 {MAX_REFERENCE_IMAGES} 张")));
    }
    for i in &images {
        if !(i.starts_with("data:image/") || i.starts_with("node-image:")) {
            return Err(GenError::Input("参考图片格式不对".into()));
        }
    }
    let mut params = serde_json::to_value(&task)?;
    if !images.is_empty() {
        params["reference_images"] = serde_json::json!(images);
    }
    let job = jobs::create_job(world.conn(), task.kind(), &task.target(), &params)?;
    prepare_job(world, &job, templates, caps, opts, provider)
}

/// 任务参数里的参考图片换成可以直接发给模型的 data URL。
fn reference_images(world: &World, params: &Value) -> Result<Vec<String>> {
    let mut out = vec![];
    for v in params.get("reference_images").and_then(Value::as_array).into_iter().flatten() {
        let Some(s) = v.as_str() else { continue };
        if let Some(id) = s.strip_prefix("node-image:") {
            out.push(chronicle_core::images::data_url(world.conn(), id)?);
        } else {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

/// 重试一个失败或取消的任务（参数不变，重新读取最新的世界内容）。
pub fn prepare_retry(
    world: &mut World,
    job_id: &str,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    provider: (&str, &str),
) -> Result<Prepared> {
    let job = jobs::get_job(world.conn(), job_id)?;
    if !matches!(job.status, JobStatus::Failed | JobStatus::Cancelled) {
        return Err(GenError::Input("只有失败或已取消的任务可以重试".into()));
    }
    prepare_job(world, &job, templates, caps, opts, provider)
}

fn prepare_job(
    world: &mut World,
    job: &GenJob,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    provider: (&str, &str),
) -> Result<Prepared> {
    let task: Task = serde_json::from_value(job.params.clone())?;
    let use_json_schema = caps.is_some_and(|c| c.json_schema);
    let images = match reference_images(world, &job.params) {
        Ok(v) => v,
        Err(e) => {
            jobs::finish(world.conn(), &job.id, JobStatus::Failed, None, Some(&format!("参考图片读取失败：{e}")))?;
            return Err(e);
        }
    };
    if !images.is_empty() && !caps.is_some_and(|c| c.vision) {
        let msg = "当前模型服务没有通过「看图」测试（或还没测过），不能使用参考图片。请到「模型服务」页点「重新测试」";
        jobs::finish(world.conn(), &job.id, JobStatus::Failed, None, Some(msg))?;
        return Err(GenError::Input(msg.into()));
    }
    let prompt = match build_prompt(world, &task, templates, opts.context_budget, !use_json_schema) {
        Ok(p) => p,
        Err(e) => {
            jobs::finish(world.conn(), &job.id, JobStatus::Failed, None, Some(&e.to_string()))?;
            return Err(e);
        }
    };
    jobs::mark_running(world.conn(), &job.id, provider.0, provider.1, &prompt.context.digest, &templates.version)?;
    if !prompt.context.dropped.is_empty() {
        jobs::set_note(world.conn(), &job.id, &format!("参考资料超出预算，已省略：{}", prompt.context.dropped.join("、")))?;
    }
    Ok(Prepared {
        job_id: job.id.clone(),
        task,
        messages: vec![
            ChatMessage::system(prompt.system),
            if images.is_empty() {
                ChatMessage::user(prompt.user)
            } else {
                let n = images.len();
                ChatMessage::user_with_images(
                    format!(
                        "{}

【参考图片】附带了 {n} 张参考图片。请把图片里能看清的内容（人物外貌与服饰、地点风貌与建筑、地图上的地名与方位、器物、氛围与色调）作为设定依据，并与上面的文字要求保持一致；看不清或无法确定的细节不要当成确定事实写进设定。",
                        prompt.user
                    ),
                    images,
                )
            },
        ],
        schema_name: prompt.schema_name.into(),
        schema: prompt.schema,
        use_json_schema,
        context_dropped: prompt.context.dropped,
    })
}

/// 模型调用失败时的错误与（如有）最后一次原始输出。
pub type CallError = (GenError, Option<String>);

/// 调用模型并取得结构正确的输出。不访问数据库。
/// 返回值中的请求记录无论成败都会给出，便于统计。
pub fn call_model(
    client: &dyn LlmClient,
    p: &Prepared,
    opts: &GenOptions,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(usize),
) -> (std::result::Result<ModelOutput, CallError>, Vec<RequestLog>) {
    let mut logs = vec![];
    let mut messages = p.messages.clone();
    let mut use_schema = p.use_json_schema;
    let mut last_raw: Option<String> = None;
    let mut repairs = 0;
    let mut thinking = opts.thinking;
    let mut thinking_disabled_on_retry = false;
    loop {
        // 实测：思考型模型在 1 万 token 的上限下会被截断。上下文已知时，把剩余空间尽量留给输出。
        let max_tokens = match opts.context_length {
            Some(ctx) => {
                let prompt: usize = messages.iter().map(|m| crate::context::estimate_tokens(&m.content) + 8).sum();
                let room = ctx.saturating_sub(prompt + 1024);
                (room as u32).clamp(512, opts.max_tokens)
            }
            None => opts.max_tokens,
        };
        let req = ChatRequest {
            messages: messages.clone(),
            temperature: Some(opts.temperature),
            max_tokens: Some(max_tokens),
            format: if use_schema {
                OutputFormat::JsonSchema { name: p.schema_name.clone(), schema: p.schema.clone() }
            } else {
                OutputFormat::Text
            },
            stream: opts.stream,
            thinking,
        };
        let resp = match client.chat(&req, cancel, progress) {
            Ok(r) => {
                logs.push(RequestLog::ok(&r));
                r
            }
            Err(LlmError::Http { status: 400, .. }) if use_schema => {
                // 服务拒绝结构化输出参数：改为在提示词中给出 schema 再试一次。
                logs.push(RequestLog { ok: false, error: Some("服务拒绝 response_format，改用提示词约束".into()), ..Default::default() });
                use_schema = false;
                if let Some(ChatMessage { content, .. }) = messages.first_mut() {
                    content.push_str(&format!("\n\n输出必须符合下面的 JSON Schema：\n{}", p.schema));
                }
                continue;
            }
            Err(e) => {
                logs.push(RequestLog::err(&e));
                let ge = if matches!(e, LlmError::Cancelled) { GenError::Cancelled } else { GenError::Llm(e) };
                return (Err((ge, last_raw)), logs);
            }
        };
        let raw = resp.content.clone();
        last_raw = Some(raw.clone());
        let truncated = resp.finish_reason.as_deref() == Some("length");
        let problem = match extract_json(&raw) {
            None if truncated => "输出因长度限制被截断，没有得到完整的 JSON；请输出更精简的内容".to_string(),
            None => "没有找到 JSON 对象".to_string(),
            Some(text) => match serde_json::from_str::<Value>(&text) {
                Err(e) => format!("JSON 语法错误：{e}"),
                Ok(v) => match p.task.check_output(&v) {
                    Ok(()) => {
                        return (Ok(ModelOutput { raw, value: v, repaired: repairs > 0, thinking_disabled_on_retry }), logs);
                    }
                    Err(e) => e,
                },
            },
        };
        if repairs >= opts.repair_attempts {
            return (Err((GenError::Output(problem), last_raw)), logs);
        }
        repairs += 1;
        // 实测：思考型模型在复杂任务上可能把整个输出上限都用在思考上。被截断时，修正请求关闭思考。
        if truncated && thinking != Some(false) {
            thinking = Some(false);
            thinking_disabled_on_retry = true;
        }
        let shown: String = raw.chars().take(6000).collect();
        messages.push(ChatMessage::assistant(shown));
        messages.push(ChatMessage::user(format!(
            "你上一次的输出无法使用：{problem}\n\n请重新输出，只输出一个符合要求的 JSON 对象，不要包含任何其他文字或代码块标记。"
        )));
    }
}

/// 把输出转换为待审变更集并保存；任务标记为成功。转换失败则任务失败，世界不变。
pub fn finalize(world: &mut World, p: &Prepared, out: &ModelOutput) -> Result<Changeset> {
    let res = convert(world, &p.task, &out.value, &p.job_id)
        .and_then(|(cs, notes)| changeset::propose_with_notes(world, cs, notes).map_err(GenError::from));
    match res {
        Ok(cs) => {
            jobs::finish(world.conn(), &p.job_id, JobStatus::Succeeded, Some(&out.raw), None)?;
            if out.thinking_disabled_on_retry {
                jobs::set_note(world.conn(), &p.job_id, "首次输出因思考过长被截断，修正时关闭了思考后成功")?;
            } else if out.repaired {
                jobs::set_note(world.conn(), &p.job_id, "首次输出无法解析，经模型修正后成功")?;
            }
            Ok(cs)
        }
        Err(e) => {
            jobs::finish(world.conn(), &p.job_id, JobStatus::Failed, Some(&out.raw), Some(&e.to_string()))?;
            Err(e)
        }
    }
}

/// 记录模型调用失败（或取消）。
pub fn fail(world: &World, job_id: &str, err: &GenError, raw: Option<&str>) -> Result<()> {
    let status = if matches!(err, GenError::Cancelled) { JobStatus::Cancelled } else { JobStatus::Failed };
    jobs::finish(world.conn(), job_id, status, raw, Some(&err.to_string()))?;
    Ok(())
}

pub struct RunOutcome {
    pub job_id: String,
    pub result: Result<Changeset>,
    pub requests: Vec<RequestLog>,
}

/// 一次性执行（测试与简单场景使用；会在调用模型期间持有 `world`）。
pub fn run(
    world: &mut World,
    client: &dyn LlmClient,
    task: Task,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    cancel: &CancelToken,
) -> Result<RunOutcome> {
    let (name, model) = client.describe();
    let p = prepare(world, task, templates, caps, opts, (&name, &model))?;
    Ok(execute(world, client, p, opts, cancel))
}

/// 同 [`run`]，附带参考图片（测试与简单场景使用）。
#[allow(clippy::too_many_arguments)]
pub fn run_with_images(
    world: &mut World,
    client: &dyn LlmClient,
    task: Task,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    cancel: &CancelToken,
    images: Vec<String>,
) -> Result<RunOutcome> {
    let (name, model) = client.describe();
    let p = prepare_with_images(world, task, templates, caps, opts, (&name, &model), images)?;
    Ok(execute(world, client, p, opts, cancel))
}

/// 重试（测试与简单场景使用）。
pub fn retry(
    world: &mut World,
    client: &dyn LlmClient,
    job_id: &str,
    templates: &Templates,
    caps: Option<&Capabilities>,
    opts: &GenOptions,
    cancel: &CancelToken,
) -> Result<RunOutcome> {
    let (name, model) = client.describe();
    let p = prepare_retry(world, job_id, templates, caps, opts, (&name, &model))?;
    Ok(execute(world, client, p, opts, cancel))
}

fn execute(world: &mut World, client: &dyn LlmClient, p: Prepared, opts: &GenOptions, cancel: &CancelToken) -> RunOutcome {
    let (res, requests) = call_model(client, &p, opts, cancel, &mut |_| {});
    let result = match res {
        Ok(out) => finalize(world, &p, &out),
        Err((e, raw)) => match fail(world, &p.job_id, &e, raw.as_deref()) {
            Ok(()) => Err(e),
            Err(e2) => Err(e2),
        },
    };
    RunOutcome { job_id: p.job_id, result, requests }
}
