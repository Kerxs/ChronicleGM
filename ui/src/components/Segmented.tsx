// 分段控件：选中块会滑动到当前项（苹果风格）。合并页面的切换都用它。
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

export interface SegItem<K extends string> {
  key: K;
  label: string;
  count?: number;
}

export function Segmented<K extends string>({ items, value, onChange }: { items: SegItem<K>[]; value: K; onChange: (k: K) => void }) {
  const box = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState<{ left: number; width: number } | null>(null);
  useLayoutEffect(() => {
    const measure = () => {
      const el = box.current?.querySelector<HTMLButtonElement>(`button[data-k="${value}"]`);
      if (el) setThumb({ left: el.offsetLeft, width: el.offsetWidth });
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (box.current) ro.observe(box.current);
    return () => ro.disconnect();
  }, [value, items.length]);
  return (
    <div className="segmented" ref={box} role="tablist">
      {thumb && <span className="seg-thumb" style={{ transform: `translateX(${thumb.left}px)`, width: thumb.width }} />}
      {items.map((it) => (
        <button key={it.key} data-k={it.key} role="tab" aria-selected={it.key === value} className={it.key === value ? "active" : ""} onClick={() => onChange(it.key)}>
          {it.label}
          {!!it.count && <span className="seg-count">{it.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** 合并后的页面：顶部大标题 + 分段切换，下面是原来的页面（原页面自己的标题隐藏）。 */
export function TabbedPage<K extends string>({ title, items, value, onChange, children }: { title: string; items: SegItem<K>[]; value: K; onChange: (k: K) => void; children: ReactNode }) {
  return (
    <div className="tabbed">
      <div className="page-head">
        <h2>{title}</h2>
        <Segmented items={items} value={value} onChange={onChange} />
      </div>
      <div key={value} className="tab-body">
        {children}
      </div>
    </div>
  );
}
