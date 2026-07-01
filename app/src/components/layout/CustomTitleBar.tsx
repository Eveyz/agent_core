import PanelLeftIcon from "lucide-react/dist/esm/icons/panel-left.mjs";
import PanelRightIcon from "lucide-react/dist/esm/icons/panel-right.mjs";
const TITLE_BAR_HEIGHT = 44;

export function CustomTitleBar({
  sidebarCollapsed,
  onToggleSidebar,
}: {
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
}) {
  return (
    <div
      style={
        {
          height: TITLE_BAR_HEIGHT,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0 8px 0 8px",
          backgroundColor: "var(--bg-sidebar)",
          borderRight: "1px solid var(--border-color)",
          userSelect: "none",
          flexShrink: 0,
          // @ts-ignore - webkit-app-region is not in React's CSSProperties type
          WebkitAppRegion: "drag",
          // @ts-ignore
          appRegion: "drag",
        } as React.CSSProperties
      }
    >
      {/* Left: window controls spacer for native traffic lights */}
      <div style={{ width: 72, flexShrink: 0 }} />

      {/* Center: flex spacer (draggable) */}
      <div style={{ flex: 1 }} />

      {/* Right: sidebar toggle */}
      <span
        onClick={onToggleSidebar}
        title={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          width: 28,
          height: 28,
          borderRadius: 6,
          cursor: "pointer",
          color: "var(--text-primary)",
          marginRight: 4,
          // @ts-ignore
          WebkitAppRegion: "no-drag",
          // @ts-ignore
          appRegion: "no-drag",
        }}
        onMouseEnter={(e) =>
          (e.currentTarget.style.background = "var(--overlay-0_1)")
        }
        onMouseLeave={(e) =>
          (e.currentTarget.style.background = "transparent")
        }
      >
        {sidebarCollapsed ? (
          <PanelRightIcon size={16} />
        ) : (
          <PanelLeftIcon size={16} />
        )}
      </span>
    </div>
  );
}
