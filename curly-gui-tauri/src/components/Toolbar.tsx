interface ToolbarProps {
  zoom: number;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomReset: () => void;
  isDark: boolean;
  onToggleTheme: () => void;
}

export function Toolbar({ zoom, onZoomIn, onZoomOut, onZoomReset, isDark, onToggleTheme }: ToolbarProps) {
  return (
    <div className="toolbar">
      <button className="toolbar-btn" onClick={onToggleTheme} title="Toggle light/dark">
        {isDark ? "🌙" : "☀"}
      </button>
      <span className="toolbar-sep" />
      <span className="weak">Zoom:</span>
      <button className="toolbar-btn" onClick={onZoomOut} title="Zoom out">
        −
      </button>
      <span className="zoom-pct">{Math.round(zoom * 100)}%</span>
      <button className="toolbar-btn" onClick={onZoomIn} title="Zoom in">
        +
      </button>
      <button className="toolbar-btn" onClick={onZoomReset} title="Reset to 100%">
        Reset
      </button>
    </div>
  );
}
