import { Plus, X } from "lucide-react";
import type { UserRule } from "../domain/types";

type RuleListProps = {
  rules: UserRule[];
  draftRule: string;
  onDraftRuleChange: (value: string) => void;
  onAddRule: () => void;
  onToggleRule: (id: string) => void;
  onRemoveRule: (id: string) => void;
};

export function RuleList({
  rules,
  draftRule,
  onDraftRuleChange,
  onAddRule,
  onToggleRule,
  onRemoveRule,
}: RuleListProps) {
  return (
    <section className="panel rules-panel" aria-labelledby="rules-title">
      <div className="panel-heading">
        <div>
          <h2 id="rules-title">长期规则</h2>
          <p>自动注入到每次生成中</p>
        </div>
      </div>

      <div className="rule-list">
        {rules.map((rule) => (
          <label className="rule-row" key={rule.id}>
            <input
              checked={rule.enabled}
              onChange={() => onToggleRule(rule.id)}
              type="checkbox"
            />
            <span>{rule.text}</span>
            <button
              aria-label={`删除规则：${rule.text}`}
              className="icon-button"
              onClick={(event) => {
                event.preventDefault();
                onRemoveRule(rule.id);
              }}
              type="button"
            >
              <X size={15} />
            </button>
          </label>
        ))}
      </div>

      <div className="rule-input-row">
        <input
          onChange={(event) => onDraftRuleChange(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              onAddRule();
            }
          }}
          placeholder="例如：默认要求列出验证方式"
          value={draftRule}
        />
        <button className="icon-button strong" onClick={onAddRule} type="button">
          <Plus size={16} />
        </button>
      </div>
    </section>
  );
}
