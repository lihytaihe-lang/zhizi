import { useEffect, useState } from "react";
import { BookOpen, ChevronRight, ListChecks, SlidersHorizontal } from "lucide-react";
import {
  contextModelAvailability,
  listQualityRecords,
  projectRuntimeConfiguration,
  confirmTestRecordingConsent,
  qualityRecordingStatus,
  testUploadEnabled,
  setContextModelRankingEnabled,
  setQualityRecording,
} from "../lib/lexicon";
import { FUZZY_PAIRS, imePreferences, setImeFuzzyMask, setImeTypoCorrection, type ImePreferences } from "../lib/imePreferences";
import { percent, weeklyStats, type WeeklyStats } from "../lib/weeklyStats";

export type HomeDestination = "lexicon" | "records" | "advanced";

type HomePanelProps = { onNavigate: (destination: HomeDestination) => void };

/** The 知字 home page (owner 2026-10-10, layout B): the toggles the owner uses daily,
 *  the pointer to the skin gallery and this week's numbers on one page; everything else is one click away. */
export function HomePanel({ onNavigate }: HomePanelProps) {
  const [ranking, setRanking] = useState(false);
  const [modelReady, setModelReady] = useState(true);
  const [recording, setRecording] = useState(false);
  const [ime, setIme] = useState<ImePreferences>({ typoCorrectionEnabled: false, fuzzyMask: 0 });
  const [stats, setStats] = useState<WeeklyStats>({ commits: 0, firstChoiceRate: null, redoRate: null });
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [uploads, setUploads] = useState(false);

  useEffect(() => {
    void projectRuntimeConfiguration().then((runtime) => {
      setRanking(runtime.contextModelRankingEnabled ?? false);
    }).catch(() => {});
    void contextModelAvailability().then((availability) => setModelReady(availability.available)).catch(() => {});
    void testUploadEnabled().then(setUploads).catch(() => {});
    void qualityRecordingStatus().then((status) => setRecording(status.enabled && status.continuous)).catch(() => {});
    void imePreferences().then(setIme).catch(() => {});
    void listQualityRecords().then((events) => setStats(weeklyStats(events, Date.now()))).catch(() => {});
  }, []);

  async function run(action: () => Promise<void>, done: string) {
    setBusy(true);
    try {
      await action();
      setMessage(done);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  const fuzzyOn = ime.fuzzyMask !== 0;
  const toggles = [
    {
      label: "顺手·知字排序", hint: modelReady ? "按上下文挑字、拼整句" : "本机模型不可用", on: ranking && modelReady,
      disabled: !modelReady,
      flip: () => run(async () => setRanking((await setContextModelRankingEnabled(!ranking)).contextModelRankingEnabled), "已保存，下次开始输入时生效。"),
    },
    {
      label: "持续记录", hint: uploads ? "测试版：每小时上传给作者" : "只存在这台机器上", on: recording, disabled: false,
      flip: () => run(async () => {
        if (!recording && !(await confirmTestRecordingConsent())) {
          throw new Error("没有开启记录。");
        }
        const status = await setQualityRecording(recording ? "off" : "continuous");
        setRecording(status.enabled && status.continuous);
      }, recording ? "已停止记录，也不再上传。" : "已开始持续记录。"),
    },
    {
      label: "手误纠错", hint: "字母打反、漏打、多打", on: ime.typoCorrectionEnabled, disabled: false,
      flip: () => run(async () => setIme(await setImeTypoCorrection(!ime.typoCorrectionEnabled)), "已保存，切换到输入框后生效。"),
    },
    {
      label: "模糊音", hint: fuzzyOn ? FUZZY_PAIRS.filter((_, bit) => ime.fuzzyMask & (1 << bit)).join(" ") : "zh/z、an/ang 等", on: fuzzyOn,
      disabled: false,
      flip: () => run(async () => setIme(await setImeFuzzyMask(fuzzyOn ? 0 : 0b111)), "已保存，切换到输入框后生效。"),
    },
  ];

  return (
    <section className="panel home-panel" aria-labelledby="home-title">
      <div className="panel-heading">
        <div>
          <h2 id="home-title">常用</h2>
          <p>常用开关、皮肤和这周的输入情况</p>
        </div>
      </div>

      <div className="home-toggles" role="group" aria-label="常用开关">
        {toggles.map((toggle) => (
          <button
            aria-pressed={toggle.on}
            className={`home-toggle ${toggle.on ? "is-on" : ""}`}
            disabled={busy || toggle.disabled}
            key={toggle.label}
            onClick={() => void toggle.flip()}
            type="button"
          >
            <strong>{toggle.label}</strong>
            <span>{toggle.on ? "开" : "关"} · {toggle.hint}</span>
          </button>
        ))}
      </div>
      {message ? <p className="home-message" role="status">{message}</p> : null}

      <h3 className="home-subtitle">皮肤</h3>
      <p className="home-message">换皮肤：点屏幕右上角菜单栏的输入法图标，选「皮肤管理…」，在窗口里挑一套、看实时预览、点「使用这套皮肤」。</p>

      <h3 className="home-subtitle">这周的输入</h3>
      <div className="home-stats">
        <div><span>上屏</span><strong>{stats.commits}</strong></div>
        <div><span>首选命中</span><strong>{percent(stats.firstChoiceRate)}</strong></div>
        <div><span>上屏后重打</span><strong>{percent(stats.redoRate)}</strong></div>
      </div>

      <nav className="home-links" aria-label="更多">
        <button onClick={() => onNavigate("lexicon")} type="button"><BookOpen size={15} />词库<ChevronRight size={15} /></button>
        <button onClick={() => onNavigate("records")} type="button"><ListChecks size={15} />记录明细<ChevronRight size={15} /></button>
        <button onClick={() => onNavigate("advanced")} type="button"><SlidersHorizontal size={15} />高级<ChevronRight size={15} /></button>
      </nav>
    </section>
  );
}
