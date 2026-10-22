import { Clock, Copy, Star, Trash2 } from "lucide-react";
import type { HistoryItem } from "../domain/types";

type HistoryListProps = {
  history: HistoryItem[];
  onCopy: (output: string) => void;
  onRemove: (id: string) => void;
  onToggleFavorite: (id: string) => void;
};

export function HistoryList({
  history,
  onCopy,
  onRemove,
  onToggleFavorite,
}: HistoryListProps) {
  return (
    <section className="panel history-panel" aria-labelledby="history-title">
      <div className="panel-heading">
        <div>
          <h2 id="history-title">最近生成</h2>
          <p>本地保存最近 50 条</p>
        </div>
        <Clock size={18} />
      </div>

      <div className="history-list">
        {history.length === 0 ? (
          <p className="empty-state">生成后会在这里留下记录。</p>
        ) : (
          history.map((item) => (
            <article className="history-item" key={item.id}>
              <div>
                <strong>{item.rawInput}</strong>
                <span>{new Date(item.createdAt).toLocaleString()}</span>
              </div>
              <div className="history-actions">
                <button
                  aria-label="收藏"
                  className={`icon-button ${item.favorite ? "is-active" : ""}`}
                  onClick={() => onToggleFavorite(item.id)}
                  type="button"
                >
                  <Star size={15} />
                </button>
                <button
                  aria-label="复制历史输出"
                  className="icon-button"
                  onClick={() => onCopy(item.output)}
                  type="button"
                >
                  <Copy size={15} />
                </button>
                <button
                  aria-label="删除历史"
                  className="icon-button"
                  onClick={() => onRemove(item.id)}
                  type="button"
                >
                  <Trash2 size={15} />
                </button>
              </div>
            </article>
          ))
        )}
      </div>
    </section>
  );
}
