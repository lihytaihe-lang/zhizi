import { Check, Copy } from "lucide-react";
import type { CopyState } from "../domain/types";

type ResultPanelProps = {
  output: string;
  copyState: CopyState;
  onCopy: () => void;
};

export function ResultPanel({ output, copyState, onCopy }: ResultPanelProps) {
  const copyLabel =
    copyState === "copied" ? "已复制" : copyState === "failed" ? "复制失败" : "复制";

  return (
    <section className="panel result-panel" aria-labelledby="result-title">
      <div className="panel-heading">
        <div>
          <h2 id="result-title">生成结果</h2>
          <p>可直接复制给目标 AI 工具</p>
        </div>
        <button className="secondary-button" disabled={!output} onClick={onCopy} type="button">
          {copyState === "copied" ? <Check size={16} /> : <Copy size={16} />}
          {copyLabel}
        </button>
      </div>

      <pre className={`result-output ${output ? "" : "is-empty"}`}>
        {output || "输入一句想法，选择模式，然后生成结构化任务指令。"}
      </pre>
    </section>
  );
}
