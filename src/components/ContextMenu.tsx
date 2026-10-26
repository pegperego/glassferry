import { useEffect, useLayoutEffect, useRef, useState } from "react";

export type MenuItem =
  | { separator: true }
  | { label: string; shortcut?: string; disabled?: boolean; danger?: boolean; onSelect: () => void };

interface Props {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}

/** Fixed-position menu, kept inside the window; closes on outside click, Escape or scroll. */
export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPosition({
      left: Math.max(4, Math.min(x, window.innerWidth - width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - height - 4)),
    });
    el.focus();
  }, [x, y]);

  useEffect(() => {
    const close = (e: Event) => {
      if (e.type === "mousedown" && ref.current?.contains(e.target as Node)) return;
      onClose();
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    window.addEventListener("wheel", close, { passive: true });
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("wheel", close);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="context-menu"
      role="menu"
      tabIndex={-1}
      style={position}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
        e.stopPropagation();
      }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item, i) =>
        "separator" in item ? (
          <hr key={i} />
        ) : (
          <button
            key={i}
            role="menuitem"
            className={item.danger ? "danger" : undefined}
            disabled={item.disabled}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            <span>{item.label}</span>
            {item.shortcut && <kbd>{item.shortcut}</kbd>}
          </button>
        ),
      )}
    </div>
  );
}
