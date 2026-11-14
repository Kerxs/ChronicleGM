// 托管模型启动参数的可视化编辑：按分组显示常用参数（不写 = 程序默认），
// 大语言模型额外显示内存估算与建议。目录里没有的参数原样保留，在「全部参数」里编辑。
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { MemoryAdvice } from "../bindings/MemoryAdvice";
import type { ModelProfile } from "../bindings/ModelProfile";
import { catalogFor, getFlag, groups, setFlag, unknownArgs, type FlagSpec } from "../lib/launchFlags";

const gb = (mb: number) => `${(mb / 1024).toFixed(1)} GB`;

export default function LaunchArgs({ profile, onArgs }: { profile: ModelProfile; onArgs: (args: string[]) => void }) {
  const catalog = catalogFor(profile.exe, profile.role);
  const args = profile.args;
  const [raw, setRaw] = useState(false);
  const extra = unknownArgs(args, catalog);
  const set = (s: FlagSpec, v: string | null) => onArgs(setFlag(args, s, v));

  return (
    <div className="launch-args">
      {catalog.length === 0 && <p className="hint">没有认出这个程序的参数（目前认识 llama-server 与 breeze-server），请在下面的「全部参数」里编辑。</p>}
      {groups(catalog).map(([g, specs]) => (
        <fieldset key={g} className="arg-group">
          <legend>{g}</legend>
          {specs.map((s) => (
            <ArgRow key={s.key} spec={s} value={getFlag(args, s)} onChange={(v) => set(s, v)} />
          ))}
        </fieldset>
      ))}
      {profile.role === "llm" && catalog.length > 0 && <MemoryPanel profile={profile} onArgs={onArgs} />}
      <div className="row wrap">
        <button type="button" className="small link" onClick={() => setRaw((x) => !x)}>
          {raw ? "收起全部参数" : `全部参数（高级）${extra.length ? ` · 另有 ${extra.length} 项不在上面` : ""}`}
        </button>
        {!raw && extra.length > 0 && <small className="hint">{extra.join(" ")}</small>}
      </div>
      {raw && (
        <label>
          全部启动参数（每行一个，和上面的选项是同一份）
          <textarea rows={8} value={args.join("\n")} onChange={(e) => onArgs(e.target.value.split("\n").map((x) => x.trim()).filter(Boolean))} />
        </label>
      )}
    </div>
  );
}

function ArgRow({ spec, value, onChange }: { spec: FlagSpec; value: string | null; onChange: (v: string | null) => void }) {
  const on = value !== null;
  const id = `arg-${spec.key}`;
  let control: React.ReactNode;
  if (spec.kind === "switch") {
    control = <input id={id} type="checkbox" checked={on} onChange={(e) => onChange(e.target.checked ? "on" : null)} />;
  } else if (spec.kind === "choice") {
    control = (
      <select id={id} value={value ?? ""} onChange={(e) => onChange(e.target.value || null)}>
        <option value="">默认（{spec.fallback}）</option>
        {spec.choices?.map(([v, t]) => (
          <option key={v} value={v}>
            {t}
          </option>
        ))}
        {value && !spec.choices?.some(([v]) => v === value) && <option value={value}>{value}</option>}
      </select>
    );
  } else {
    control = (
      <span className="arg-input">
        <input
          id={id}
          value={value ?? ""}
          placeholder={`默认：${spec.fallback}`}
          inputMode={spec.kind === "int" ? "numeric" : undefined}
          onChange={(e) => onChange(e.target.value.trim() === "" ? null : e.target.value.trim())}
        />
        {spec.unit && <small className="hint">{spec.unit}</small>}
        {spec.kind === "path" && (
          <button
            type="button"
            className="small"
            onClick={() =>
              void open({ multiple: false, filters: spec.ext ? [{ name: spec.ext, extensions: [spec.ext] }] : undefined, directory: !spec.ext }).then((p) => {
                if (typeof p === "string") onChange(p);
              })
            }
          >
            浏览…
          </button>
        )}
      </span>
    );
  }
  return (
    <div className={`arg-row${on ? " set" : ""}`}>
      <label htmlFor={id} className="arg-label" title={spec.names.join(" / ") || "第一个参数"}>
        {spec.label}
        <code>{spec.positional ? "（第一个参数）" : spec.names[0]}</code>
      </label>
      <div className="arg-control">
        {control}
        {spec.presets && spec.kind !== "switch" && (
          <span className="arg-presets">
            {spec.presets.map(([v, t]) => (
              <button type="button" key={v} className={`chip${value === v ? " active" : ""}`} onClick={() => onChange(v)}>
                {t}
              </button>
            ))}
          </span>
        )}
        {spec.help && <small className="hint">{spec.help}</small>}
      </div>
    </div>
  );
}

/** 内存：加载后 + 提示词缓存上限 + 其他托管模型，对比物理内存；放不下时给一个一键修改。 */
function MemoryPanel({ profile, onArgs }: { profile: ModelProfile; onArgs: (args: string[]) => void }) {
  const [a, setA] = useState<MemoryAdvice | null | undefined>(undefined);
  const key = JSON.stringify(profile.args);
  useEffect(() => {
    const t = setTimeout(() => {
      api.memoryAdvice(profile).then(setA).catch(() => setA(null));
    }, 400);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, profile.id]);
  if (a === undefined) return <p className="hint">正在估算内存…</p>;
  if (a === null) return <p className="hint">无法估算内存：参数里没有可读取的模型文件（-m）。</p>;
  const others = a.others.reduce((s, [, m]) => s + m, 0);
  const cache = a.cache_mb ?? 0;
  const total = a.total_ram_mb;
  const pct = (mb: number) => `${Math.min(100, (mb / total) * 100).toFixed(1)}%`;
  const cacheSpec = catalogFor(profile.exe, profile.role).find((s) => s.key === "cache_ram");
  return (
    <div className={`mem-panel${a.over ? " over" : ""}`}>
      <div className="row">
        <strong>内存</strong>
        <span className="spacer" />
        <small className="hint">物理内存 {gb(total)}</small>
      </div>
      <div className="mem-bar" role="img" aria-label="内存占用估计">
        <span className="seg base" style={{ width: pct(a.base_mb) }} title={`模型加载后 ${gb(a.base_mb)}`} />
        <span className={`seg cache${a.cache_mb === null ? " unlimited" : ""}`} style={{ width: a.cache_mb === null ? "100%" : pct(cache) }} title={a.cache_mb === null ? "提示词缓存：不限制" : `提示词缓存最多 ${gb(cache)}`} />
        <span className="seg others" style={{ width: pct(others) }} title={`其他托管模型 ${gb(others)}`} />
        <span className="reserve" style={{ left: `calc(${pct(total - a.reserve_mb)})` }} title={`给系统留 ${gb(a.reserve_mb)}`} />
      </div>
      <div className="mem-legend">
        <span><i className="base" />模型加载后 {gb(a.base_mb)}{a.base_measured ? "（实测）" : "（估算）"}</span>
        <span><i className="cache" />提示词缓存 {a.cache_mb === null ? "不限制" : a.cache_mb === 0 ? "关闭" : `最多 ${gb(cache)}`}</span>
        {a.others.map(([n, m]) => (
          <span key={n}><i className="others" />{n} {gb(m)}（实测）</span>
        ))}
        <span><i className="reserve" />给系统留 {gb(a.reserve_mb)}</span>
      </div>
      <p className={a.over ? "warn-text" : "hint"}>
        {a.peak_mb === null
          ? "缓存不限制：长时间游玩内存会一直上涨，迟早超过物理内存。"
          : a.over
            ? `长时间游玩后最多约 ${gb(a.peak_mb)}，超过了可用的 ${gb(total - a.reserve_mb)}：其他程序会被挤到硬盘上（实测语音会明显变慢）。`
            : `长时间游玩后最多约 ${gb(a.peak_mb)}，放得下。`}
      </p>
      {a.suggested_cache_mb !== null && cacheSpec && (
        <div className="row wrap">
          <button type="button" className="primary small" onClick={() => onArgs(setFlag(profile.args, cacheSpec, String(a.suggested_cache_mb)))}>
            {a.suggested_cache_mb === 0 ? "关闭提示词缓存（--cache-ram 0）" : `把提示词缓存上限设为 ${a.suggested_cache_mb} MiB`}
          </button>
          <small className="hint">改的是这里的参数，点「保存」后下次启动模型生效。实测：设为 2048 后内存涨到约 2.3 GB 就不再涨；默认 8192 时每换一段新提示词涨约 0.24 GB。</small>
        </div>
      )}
      {a.notes.map((n, i) => (
        <small key={i} className="hint block">· {n}</small>
      ))}
    </div>
  );
}
