import { Fragment, isValidElement, useRef, type PointerEvent, type ReactNode } from "react";

const MIN_PX = 240;

interface Props {
  direction: "row" | "column";
  /** Relative sizes (flex-grow), one per child; missing or invalid ones mean equal parts. */
  sizes: number[];
  onSizes: (sizes: number[]) => void;
  children: ReactNode[];
}

/** Children side by side (row) or stacked (column) with draggable dividers. */
export function SplitView({ direction, sizes, onSizes, children }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const valid = sizes.length === children.length && sizes.every((s) => Number.isFinite(s) && s > 0);
  const current = valid ? sizes : children.map(() => 1);

  const startDrag = (index: number, e: PointerEvent<HTMLDivElement>) => {
    const container = ref.current;
    if (!container) return;
    e.preventDefault();
    const horizontal = direction === "row";
    const total = current.reduce((a, b) => a + b, 0);
    const extent = horizontal ? container.clientWidth : container.clientHeight;
    const unit = total / extent; // flex units per pixel
    const start = horizontal ? e.clientX : e.clientY;
    const [a, b] = [current[index], current[index + 1]];
    const min = Math.min(MIN_PX * unit, (a + b) / 2);
    const move = (ev: globalThis.PointerEvent) => {
      const delta = ((horizontal ? ev.clientX : ev.clientY) - start) * unit;
      const left = Math.min(a + b - min, Math.max(min, a + delta));
      const next = [...current];
      next[index] = left;
      next[index + 1] = a + b - left;
      onSizes(next);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove("resizing");
    };
    document.body.classList.add("resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div ref={ref} className={`split split-${direction}`}>
      {children.map((child, i) => (
        // The child's own key keeps each pane's state when another one appears or goes.
        <Fragment key={isValidElement(child) && child.key !== null ? child.key : i}>
          {i > 0 && (
            <div
              className="splitter"
              role="separator"
              aria-orientation={direction === "row" ? "vertical" : "horizontal"}
              onPointerDown={(e) => startDrag(i - 1, e)}
              onDoubleClick={() => onSizes(children.map(() => 1))}
              title="Trascina per ridimensionare, doppio clic per dividere in parti uguali"
            />
          )}
          <div className="split-cell" style={{ flexGrow: current[i] }}>
            {child}
          </div>
        </Fragment>
      ))}
    </div>
  );
}
