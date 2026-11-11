import { useEffect, useRef, useState } from "react";

/** A "⋯" button that opens a small popup menu, visible only while the row
 * (its `.tree-row` ancestor) is hovered — plain CSS `:hover` here, unlike
 * the egui GUI's `hover_ellipsis_row`, which had to fake it with a
 * one-frame-lag `ctx.data`/`request_repaint` trick since egui has no
 * live `:hover`. Closes on an outside click or after an item is chosen. */
export function RowMenu({ actions }: { actions: { label: string; onClick: () => void }[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function onDocClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [open]);

  return (
    <div className="row-menu" ref={ref} onClick={(e) => e.stopPropagation()}>
      <button className={open ? "row-menu-btn menu-open" : "row-menu-btn"} onClick={() => setOpen((o) => !o)}>
        ⋯
      </button>
      {open && (
        <div className="row-menu-popup">
          {actions.map((a) => (
            <button
              key={a.label}
              onClick={() => {
                setOpen(false);
                a.onClick();
              }}
            >
              {a.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
