import { useEffect, useState } from "react";
import { FolderKanban, Power, Save, Trash2 } from "lucide-react";
import {
  activateProjectProfile, deleteProjectProfile, listProjectProfiles,
  projectRuntimeConfiguration, saveProjectProfile, setPredictionEnabled,
  setSecondaryLayerEnabled, splitLexiconList,
  type ProjectProfile, type ProjectRuntimeConfiguration,
} from "../lib/lexicon";

const emptyRuntime: ProjectRuntimeConfiguration = {
  activeProfileId: null, activeProjectDomain: null, activeProfessionalPacks: [],
  agentContext: "", secondaryLayerEnabled: true, predictionEnabled: false, contextModelRankingEnabled: false, candidateTheme: "system",
};

export function ProjectProfileManager() {
  const [profiles, setProfiles] = useState<ProjectProfile[]>([]);
  const [runtime, setRuntime] = useState(emptyRuntime);
  const [draft, setDraft] = useState({ id: "", name: "", projectDomain: "", packs: "", agentContext: "" });
  const [notice, setNotice] = useState("");

  async function refresh(nextRuntime?: ProjectRuntimeConfiguration) {
    const nextProfiles = await listProjectProfiles();
    setProfiles(nextProfiles);
    const active = nextRuntime ?? (nextProfiles.find((profile) => profile.active)
      ? {
          activeProfileId: nextProfiles.find((profile) => profile.active)!.id,
          activeProjectDomain: nextProfiles.find((profile) => profile.active)!.projectDomain,
          activeProfessionalPacks: nextProfiles.find((profile) => profile.active)!.professionalPacks,
          agentContext: nextProfiles.find((profile) => profile.active)!.agentContext,
          secondaryLayerEnabled: runtime.secondaryLayerEnabled,
          predictionEnabled: runtime.predictionEnabled,
          contextModelRankingEnabled: runtime.contextModelRankingEnabled,
          candidateTheme: runtime.candidateTheme,
        }
      : { ...runtime, activeProfileId: null, activeProjectDomain: null, activeProfessionalPacks: [], agentContext: "" });
    setRuntime(active);
  }

  useEffect(() => {
    void Promise.all([listProjectProfiles(), projectRuntimeConfiguration()])
      .then(([nextProfiles, nextRuntime]) => { setProfiles(nextProfiles); setRuntime(nextRuntime); })
      .catch((error) => setNotice(String(error)));
  }, []);

  async function save() {
    if (!draft.name.trim() || !draft.projectDomain.trim()) {
      setNotice("项目名称和项目域为必填。"); return;
    }
    try {
      const now = Date.now();
      await saveProjectProfile({
        id: draft.id.trim() || `project-${now}`,
        name: draft.name.trim(),
        projectDomain: draft.projectDomain.trim(),
        professionalPacks: splitLexiconList(draft.packs),
        agentContext: draft.agentContext.trim(),
        updatedAtMs: now,
      });
      setDraft({ id: "", name: "", projectDomain: "", packs: "", agentContext: "" });
      setNotice("Project Profile 已保存。");
      await refresh();
    } catch (error) { setNotice(`保存失败：${String(error)}`); }
  }

  async function activate(id: string | null, name?: string) {
    try {
      const next = await activateProjectProfile(id);
      setRuntime(next); await refresh(next);
      setNotice(id === null ? "已切换为无项目：全局稳定排序。" : `已激活 ${name}；重新激活输入法后生效。`);
    } catch (error) { setNotice(`切换失败：${String(error)}`); }
  }

  async function remove(id: string) {
    try { await deleteProjectProfile(id); await refresh(); setNotice("Project Profile 已删除。"); }
    catch (error) { setNotice(`删除失败：${String(error)}`); }
  }

  async function toggleSecondary(enabled: boolean) {
    try {
      const next = await setSecondaryLayerEnabled(enabled); setRuntime(next);
      setNotice(enabled ? "第二层候选已开启。" : "第二层候选已整体关闭。");
    } catch (error) { setNotice(`切换失败：${String(error)}`); }
  }

  async function togglePrediction(enabled: boolean) {
    try {
      const next = await setPredictionEnabled(enabled); setRuntime(next);
      setNotice(enabled ? "本地关联预测已开启；发布第二层快照后生效。" : "本地关联预测已关闭。");
    } catch (error) { setNotice(`切换失败：${String(error)}`); }
  }

  return <section className="project-profile-section" aria-labelledby="project-profile-title">
    <div className="settings-section-heading"><FolderKanban size={18} /><div><h3 id="project-profile-title">Project Profile</h3><p>切换项目会激活项目域、专业词包和 Agent 上下文；“无项目”保持全局稳定排序</p></div></div>
    <div className="project-profile-grid">
      <div className="project-profile-editor">
        <label>项目名称<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.currentTarget.value })} /></label>
        <label>项目域<input placeholder="例如 agent-input-method" value={draft.projectDomain} onChange={(event) => setDraft({ ...draft, projectDomain: event.currentTarget.value })} /></label>
        <label>自动激活词包<input placeholder="逗号分隔" value={draft.packs} onChange={(event) => setDraft({ ...draft, packs: event.currentTarget.value })} /></label>
        <label>Agent 上下文<textarea rows={3} value={draft.agentContext} onChange={(event) => setDraft({ ...draft, agentContext: event.currentTarget.value })} /></label>
        <button className="primary-button" type="button" onClick={() => void save()}><Save size={15} />保存 Profile</button>
      </div>
      <div className="project-profile-list">
        <button className={`profile-card ${runtime.activeProfileId === null ? "is-active" : ""}`} type="button" onClick={() => void activate(null)}><Power size={15} /><span><strong>无项目</strong><small>不激活项目词或专业词包</small></span></button>
        {profiles.map((profile) => <div className={`profile-card ${profile.id === runtime.activeProfileId ? "is-active" : ""}`} key={profile.id}>
          <button className="profile-select" type="button" onClick={() => void activate(profile.id, profile.name)}><strong>{profile.name}</strong><small>{profile.projectDomain} · {profile.professionalPacks.join("、") || "无词包"}</small></button>
          <button className="icon-button" aria-label={`删除 ${profile.name}`} type="button" onClick={() => void remove(profile.id)}><Trash2 size={14} /></button>
        </div>)}
      </div>
    </div>
    <label className="toggle-row"><input type="checkbox" checked={runtime.secondaryLayerEnabled} onChange={(event) => void toggleSecondary(event.currentTarget.checked)} /><span>开启第二层扩展候选（不占数字键）</span></label>
    <label className="toggle-row"><input type="checkbox" checked={runtime.predictionEnabled} onChange={(event) => void togglePrediction(event.currentTarget.checked)} /><span>开启纯本地关联预测（默认关闭，仅第二层）</span></label>
    {runtime.agentContext ? <div className="active-agent-context"><strong>当前 Agent 上下文</strong><pre>{runtime.agentContext}</pre></div> : null}
    {notice ? <p className="settings-message" role="status">{notice}</p> : null}
  </section>;
}
