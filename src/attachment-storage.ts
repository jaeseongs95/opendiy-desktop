import { bindAttribute, bindText, locale, onLanguageChange, t } from './i18n';
import './attachment-storage.css';

interface StorageSession { sessionId: string; bytes: number; files: number }
export interface AttachmentStorageUsage {
  totalBytes: number; totalFiles: number; limitBytes: number;
  draftBytes: number; draftFiles: number; retainedBytes: number; retainedFiles: number;
  otherBytes: number; otherFiles: number; draftSessions: StorageSession[]; omittedSessions: number;
}

function checkedUsage(value: AttachmentStorageUsage): AttachmentStorageUsage {
  const numbers = ['totalBytes', 'totalFiles', 'limitBytes', 'draftBytes', 'draftFiles', 'retainedBytes', 'retainedFiles', 'otherBytes', 'otherFiles', 'omittedSessions'] as const;
  if (!value || numbers.some(key => !Number.isSafeInteger(value[key]) || value[key] < 0)
    || !Array.isArray(value.draftSessions) || value.draftSessions.length > 100
    || value.draftSessions.some(row => !row || typeof row.sessionId !== 'string' || row.sessionId.length > 128
      || !Number.isSafeInteger(row.bytes) || row.bytes < 0 || !Number.isSafeInteger(row.files) || row.files < 0)
    || value.totalBytes !== value.draftBytes + value.retainedBytes + value.otherBytes
    || value.totalFiles !== value.draftFiles + value.retainedFiles + value.otherFiles) {
    throw new Error(t('첨부 저장 공간 응답이 올바르지 않습니다.', 'Invalid attachment storage response.'));
  }
  return value;
}

export function createAttachmentStorage(
  call: <T>(command: string, args?: Record<string, unknown>) => Promise<T>,
  sessionLabel: (id: string) => string | undefined = () => undefined,
) {
  const dialog = document.createElement('dialog');
  dialog.id = 'attachment-storage-dialog';
  dialog.setAttribute('aria-labelledby', 'attachment-storage-title');
  dialog.innerHTML = `<div class="dialog-heading"><h2 id="attachment-storage-title"></h2><button id="attachment-storage-close" class="icon-button">×</button></div>
    <p id="attachment-storage-scope"></p><div id="attachment-storage-summary"></div>
    <p id="attachment-storage-status" role="status" aria-live="polite"></p>
    <div id="attachment-storage-error" class="inline-error" role="alert" hidden></div>
    <h3 id="attachment-storage-drafts-title"></h3><ul id="attachment-storage-drafts"></ul>
    <p id="attachment-storage-guidance"></p><p id="attachment-storage-preservation"></p>
    <div class="dialog-actions"><button id="attachment-storage-refresh" class="quiet-button"></button></div>`;
  document.body.append(dialog);
  const $ = <T extends HTMLElement>(selector: string) => dialog.querySelector<T>(selector)!;
  const summary = $('#attachment-storage-summary');
  const drafts = $('#attachment-storage-drafts');
  const status = $('#attachment-storage-status');
  const error = $('#attachment-storage-error');
  const refresh = $<HTMLButtonElement>('#attachment-storage-refresh');
  const close = $<HTMLButtonElement>('#attachment-storage-close');
  bindText($('#attachment-storage-title'), '첨부 저장 공간', 'Attachment storage');
  bindAttribute(close, 'aria-label', '첨부 저장 공간 닫기', 'Close attachment storage');
  bindText($('#attachment-storage-scope'), '앱이 복사해 보관한 이미지만 집계합니다. 직접 선택한 원본 로컬 파일은 포함하지 않습니다.', 'Only images copied into app storage are counted. Original local files selected by you are excluded.');
  bindText($('#attachment-storage-drafts-title'), '현재 실행의 미전송 초안', 'Unsent drafts in this app process');
  bindText($('#attachment-storage-guidance'), '초안 이미지를 정리하려면 해당 세션에서 첨부를 제거하세요. 이 창에서는 활성 초안의 이미지를 삭제하지 않습니다.', 'To free draft space, remove the attachment in its original session. This window does not delete images from active drafts.');
  bindText($('#attachment-storage-preservation'), '전송에 사용됐거나 결과가 불명확한 이미지는 대화 이력을 위해 보존합니다. 이전 실행 또는 다른 실행의 파일은 소유 상태를 확인할 수 없어 정리 대상으로 취급하지 않습니다.', 'Images submitted for sending or with an uncertain result are preserved for conversation history. Files from previous or other app processes have unknown ownership and are not cleanup candidates.');
  bindText(refresh, '새로고침', 'Refresh');
  let version = 0;
  let pending = false;
  let usage: AttachmentStorageUsage | undefined;
  const size = (bytes: number) => `${(bytes / 1024 / 1024).toLocaleString(locale(), { maximumFractionDigits: 2 })} MiB (${bytes.toLocaleString(locale())} ${t('바이트', 'bytes')})`;
  function render() {
    summary.replaceChildren(); drafts.replaceChildren();
    if (!usage || !dialog.open) return;
    const rows: [string, string, number, number][] = [
      ['전체 사용량', 'Total usage', usage.totalBytes, usage.totalFiles],
      ['미전송 초안 · 현재 실행', 'Unsent drafts · current process', usage.draftBytes, usage.draftFiles],
      ['보존 · 현재 실행', 'Retained · current process', usage.retainedBytes, usage.retainedFiles],
      ['기타 · 이전 또는 다른 실행 포함', 'Other · includes previous or other processes', usage.otherBytes, usage.otherFiles],
    ];
    const list = document.createElement('dl');
    for (const [ko, en, bytes, files] of rows) {
      const name = document.createElement('dt'); name.textContent = t(ko, en);
      const value = document.createElement('dd'); value.textContent = `${size(bytes)} · ${files.toLocaleString(locale())}${t('개', ' files')}`;
      list.append(name, value);
    }
    const limit = document.createElement('p'); limit.id = 'attachment-storage-limit';
    limit.textContent = `${t('저장 상한', 'Storage limit')}: ${size(usage.limitBytes)}`;
    if (usage.totalBytes >= usage.limitBytes) limit.textContent += t(' · 상한에 도달했습니다.', ' · Limit reached.');
    summary.append(list, limit);
    for (const row of usage.draftSessions) {
      const item = document.createElement('li');
      const title = sessionLabel(row.sessionId)?.slice(0, 200) || row.sessionId;
      item.textContent = `${title} · ${row.files.toLocaleString(locale())}${t('개', ' files')} · ${size(row.bytes)}`;
      drafts.append(item);
    }
    if (!usage.draftSessions.length) {
      const item = document.createElement('li'); item.textContent = t('현재 추적 중인 미전송 이미지가 없습니다.', 'No tracked unsent images in this process.'); drafts.append(item);
    }
    if (usage.omittedSessions) {
      const item = document.createElement('li');
      item.textContent = t(`외 ${usage.omittedSessions}개 세션의 초안이 있습니다. 전체 사용량에는 포함됩니다.`, `Drafts in ${usage.omittedSessions} more sessions are included in the totals.`);
      drafts.append(item);
    }
  }
  function clear() {
    version++; usage = undefined; summary.replaceChildren(); drafts.replaceChildren();
    status.textContent = ''; error.textContent = ''; error.hidden = true;
  }
  function dismiss() { clear(); dialog.close(); }
  close.onclick = dismiss;
  dialog.oncancel = event => { event.preventDefault(); dismiss(); };
  dialog.onclose = () => { if (!dialog.open) clear(); };
  onLanguageChange(() => { if (dialog.open) render(); });
  async function load() {
    if (!dialog.open || pending) return;
    const token = version;
    pending = true; refresh.disabled = true; error.hidden = true;
    dialog.setAttribute('aria-busy', 'true');
    bindText(status, '용량을 확인하는 중…', 'Checking storage…');
    try {
      const result = await call<AttachmentStorageUsage>('conversation_attachment_storage');
      if (dialog.open && version === token) { usage = checkedUsage(result); render(); bindText(status, '현재 저장 상태입니다. 변경 후 새로고침하세요.', 'Storage snapshot. Refresh after changes.'); }
    } catch {
      if (dialog.open && version === token) {
        usage = undefined; render(); bindText(status, '용량을 확인하지 못했습니다.', 'Could not check storage.');
        bindText(error, '첨부 저장 공간을 확인하지 못했습니다. 잠시 후 새로고침하세요.', 'Could not check attachment storage. Please refresh shortly.'); error.hidden = false;
      }
    } finally {
      pending = false; refresh.disabled = false; dialog.setAttribute('aria-busy', 'false');
      // A reopened popup gets a fresh snapshot, never a late previous snapshot.
      if (dialog.open && version !== token) void load();
    }
  }
  refresh.onclick = () => void load();
  return {
    open() {
      if (dialog.open) return;
      clear(); dialog.showModal(); close.focus(); void load();
    },
  };
}
