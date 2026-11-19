//! 生成在线提示词包（发布提示词更新用，不用发新版软件）。
//!
//! 用法：`cargo run -p chronicle-gen --example make_prompt_pack -- <模板文件夹> <版本号> [说明] > prompt-pack.json`
//! - 模板文件夹里放改过的 `<模板名>.j2`（和软件自带的一样的不会放进包里）；
//! - 版本号必须大于软件自带的提示词版本（templates.rs 的 PROMPT_VERSION）；
//! - 把生成的 prompt-pack.json 上传到 GitHub 发布 `prompts`（替换旧文件），用户的软件会自动检查到。
use std::collections::BTreeMap;

use chronicle_gen::templates::{PROMPT_VERSION, PromptPack, builtin_sources};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法：make_prompt_pack <模板文件夹> <版本号> [说明]");
        std::process::exit(2);
    }
    let dir = std::path::Path::new(&args[1]);
    let version: u32 = args[2].parse().expect("版本号要是整数");
    if version <= PROMPT_VERSION {
        eprintln!("版本号要大于软件自带的提示词版本 v{PROMPT_VERSION}");
        std::process::exit(2);
    }
    let mut templates = BTreeMap::new();
    for (name, builtin) in builtin_sources() {
        if let Ok(src) = std::fs::read_to_string(dir.join(format!("{name}.j2")))
            && src != builtin
        {
            templates.insert(name.to_string(), src);
        }
    }
    if templates.is_empty() {
        eprintln!("文件夹里没有和自带版本不同的模板");
        std::process::exit(2);
    }
    let date = {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
        let z = secs / 86400 + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + if m <= 2 { 1 } else { 0 })
    };
    let mut p = PromptPack { version, date, notes: args.get(3).cloned().unwrap_or_default(), templates, checksum: String::new() };
    p.checksum = p.compute_checksum();
    p.validate().expect("提示词包检查不通过");
    eprintln!("提示词包 v{version}：{} 个模板（{}）", p.templates.len(), p.templates.keys().cloned().collect::<Vec<_>>().join("、"));
    println!("{}", serde_json::to_string_pretty(&p).unwrap());
}
