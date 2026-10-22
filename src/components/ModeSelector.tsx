import type { Mode, ModeId } from "../domain/types";

type ModeSelectorProps = {
  modes: Mode[];
  selectedModeId: ModeId;
  onSelect: (modeId: ModeId) => void;
};

export function ModeSelector({ modes, selectedModeId, onSelect }: ModeSelectorProps) {
  return (
    <div className="mode-grid" aria-label="Prompt modes">
      {modes.map((mode) => (
        <button
          className={`mode-option ${mode.id === selectedModeId ? "is-selected" : ""}`}
          key={mode.id}
          onClick={() => onSelect(mode.id)}
          type="button"
        >
          <span>{mode.name}</span>
          <small>{mode.description}</small>
        </button>
      ))}
    </div>
  );
}
