import AlertTriangleIcon from "lucide-react/dist/esm/icons/alert-triangle.mjs";
import InfoIcon from "lucide-react/dist/esm/icons/info.mjs";
import "./ErrorBlock.css";

export function ErrorBlock({ error }: { error: string }) {
  const isMaxIterations = error.toLowerCase().includes("max_iterations") || 
                          error.toLowerCase().includes("max iterations");

  return (
    <div className="error-block-container">
      <div className="error-block-title">
        <AlertTriangleIcon size={14} /> Execution Error
      </div>
      
      {isMaxIterations && (
        <div className="error-block-warning">
          <InfoIcon size={14} color="var(--warning)" style={{ flexShrink: 0, marginTop: "2px" }} />
          <div className="error-block-warning-text">
            <strong>Max Iterations Exceeded:</strong> Agent reached the maximum number of reasoning steps without returning a final output. You can increase the "Max Iterations" limit in the Node Properties panel.
          </div>
        </div>
      )}

      <div className="error-block-stack">
        {error}
      </div>
    </div>
  );
}
