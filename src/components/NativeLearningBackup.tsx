import {useCallback, useEffect, useState} from 'react';
import {downloadJSON} from '../lib/lexicon';
import {
  nativeLearningStatus,
  exportNativeLearning,
  previewNativeLearningImport,
  mergeNativeLearningImport,
  type NativeLearningStatus,
  type NativeLearningImportPreview,
} from '../lib/maintenance';

const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

const ONE_MIB = 1024 * 1024;

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} 字节`;
  }
  if (bytes < ONE_MIB) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / ONE_MIB).toFixed(2)} MiB`;
}

export function NativeLearningBackup({onChanged, refreshVersion = 0}: {onChanged?: () => Promise<void>; refreshVersion?: number}) {
  const [status, setStatus] = useState<NativeLearningStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [text, setText] = useState('');
  const [preview, setPreview] = useState<NativeLearningImportPreview | null>(null);
  const [previewSource, setPreviewSource] = useState('');
  const [mergeResult, setMergeResult] = useState<{words: number; choices: number} | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<'export' | 'preview' | 'merge' | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      const next = await nativeLearningStatus();
      setStatus(next);
      setLoadError(null);
    } catch (error) {
      setStatus(null);
      setLoadError(`无法读取本地学习状态：${errorText(error)}`);
    }
  }, []);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus, refreshVersion]);

  const previewValid = preview !== null && previewSource === text;

  const percent = status !== null && status.maxBytes > 0
    ? Math.min(100, Math.round((status.estimatedBytes / status.maxBytes) * 100))
    : 0;

  const handleExport = async () => {
    setBusy('export');
    setError(null);
    setMergeResult(null);
    try {
      const content = await exportNativeLearning();
      downloadJSON('知字-学习备份.json', JSON.parse(content));
    } catch (error) {
      setError(`导出失败：${errorText(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const handlePreview = async () => {
    setBusy('preview');
    setError(null);
    setMergeResult(null);
    try {
      const result = await previewNativeLearningImport(text);
      setPreview(result);
      setPreviewSource(text);
    } catch (error) {
      setPreview(null);
      setPreviewSource('');
      setError(`校验未通过：${errorText(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const handleMerge = async () => {
    setBusy('merge');
    setError(null);
    try {
      const result = await mergeNativeLearningImport(text);
      setMergeResult(result);
      setPreview(null);
      setPreviewSource('');
      setText('');
      try {
        await loadStatus();
        await onChanged?.();
      } catch {
        setLoadError('合并已完成，但刷新学习状态失败，请稍后重试。');
      }
    } catch (error) {
      setError(`合并失败：${errorText(error)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <article className="maintenance-card">
      <h3>本地学习备份</h3>
      <p>
        这里是当前正在使用的本地学习内容。备份文件包含本地积累的词条与选择记录，
        不包含服务密钥。词条和应用选择记录属于个人数据，请妥善保存备份。
      </p>
      {loadError !== null ? (
        <p role="alert">{loadError}</p>
      ) : status === null ? (
        <p>正在读取本地学习状态……</p>
      ) : (
        <ul>
          <li>词条 {status.entries} 条，选择记录 {status.choices} 条。</li>
          <li>实际文件大小 {formatBytes(status.actualBytes)}（上限 1 MiB）。</li>
          <li>
            按保守估算占用 {formatBytes(status.estimatedBytes)} / {formatBytes(status.maxBytes)}
            （约 {percent}%）。
          </li>
          <li>词条和选择记录各最多 {status.maxEntries} 条；距最近更新超过 {status.maxAgeDays} 天的记录会过期。</li>
        </ul>
      )}
      <div>
        <button type="button" onClick={handleExport} disabled={busy !== null}>
          {busy === 'export' ? '正在导出……' : '导出备份'}
        </button>
        <p>导出的备份包含全部本地词条与选择记录。</p>
      </div>
      <div>
        <textarea
          aria-label="学习备份内容"
          rows={6}
          value={text}
          placeholder="把备份文件的内容粘贴到这里"
          disabled={busy !== null}
          onChange={(event) => {
            setText(event.target.value);
            setPreview(null);
            setPreviewSource('');
            setError(null);
            setMergeResult(null);
          }}
        />
        <button
          type="button"
          onClick={handlePreview}
          disabled={busy !== null || text.trim() === ''}
        >
          {busy === 'preview' ? '正在校验……' : '预览校验'}
        </button>
        {previewValid && preview !== null ? (
          <div>
            <p>
              校验通过：该备份包含词条 {preview.entries} 条、选择记录 {preview.choices} 条，
              合并后预计占用 {formatBytes(preview.estimatedBytes)}。
            </p>
            <p>
              确认合并后，将把备份内容并入当前本地学习：同一词条按次数较多或时间较新的记录为准，
              现有数据不会被删除。
            </p>
            <button type="button" onClick={handleMerge} disabled={busy !== null}>
              {busy === 'merge' ? '正在合并……' : '确认合并'}
            </button>
          </div>
        ) : null}
      </div>
      {error !== null ? <p role="alert">{error}</p> : null}
      {mergeResult !== null ? (
        <p>合并完成：处理词条 {mergeResult.words} 条、选择记录 {mergeResult.choices} 条。</p>
      ) : null}
    </article>
  );
}
