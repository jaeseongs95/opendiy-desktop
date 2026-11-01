import { test, expect, type Page } from '@playwright/test';

async function workspace(page: Page, mode: 'normal' | 'deferred' | 'error' | 'oversized' = 'normal') {
  await page.addInitScript(({ mode }) => {
    const state = window as any;
    state.isTauri = true; state.storageCalls = []; state.storageMode = mode;
    let callback = 0;
    const snapshot = {
      totalBytes: 60, totalFiles: 6, limitBytes: 268435456,
      draftBytes: 10, draftFiles: 1, retainedBytes: 20, retainedFiles: 2,
      otherBytes: 30, otherFiles: 3, draftSessions: [{ sessionId: 'draft-session', bytes: 10, files: 1 }], omittedSessions: 0,
    };
    state.__TAURI_INTERNALS__ = {
      transformCallback: () => ++callback, unregisterCallback() {},
      async invoke(command: string, args: any = {}) {
        state.storageCalls.push({ command, args });
        if (command.startsWith('plugin:event|')) return callback;
        if (command === 'get_ui_language') return 'ko';
        if (command === 'set_ui_language') return { show: 'OpenDIY Desktop 열기', exit: '종료…' };
        if (command === 'load_layout') return { version: 1, columns: 2, sessions: [] };
        if (['list_projects', 'list_sessions', 'list_external'].includes(command)) return [];
        if (command === 'get_codex_settings') return { savedPath: null, resolvedPath: 'D:\\tools\\codex.exe', source: 'path', error: null };
        if (command === 'weekly_usage') return { executable: 'D:\\tools\\codex.exe', observedAt: Date.now(), limitName: '시험', weekly: { usedPercent: 25, resetsAt: 1900000000 } };
        if (command === 'conversation_attachment_storage') {
          if (state.storageMode === 'error') throw new Error('storage lock OS_ERROR_32 D:\\private\\attachments secret-token=fixture');
          if (state.storageMode === 'oversized') return { ...snapshot, draftSessions: Array.from({ length: 101 }, () => snapshot.draftSessions[0]) };
          if (state.storageMode === 'deferred') return new Promise(resolve => { state.finishStorage = (value = snapshot) => resolve(value); });
          return snapshot;
        }
        return null;
      },
    };
  }, { mode });
  await page.goto('/');
  await expect(page.locator('#app')).toHaveAttribute('aria-busy', 'false');
}

async function openStorage(page: Page) {
  await page.locator('#settings-open').click();
  await page.locator('#attachment-storage-open').click();
  await expect(page.locator('#attachment-storage-dialog')).toBeVisible();
}

const queries = (page: Page) => page.evaluate(() => (window as any).storageCalls.filter((call: any) => call.command === 'conversation_attachment_storage').length);

test('열 때만 용량을 조회하고 미전송·보존·기타 및 상한을 표시한다', async ({ page }) => {
  await workspace(page);
  expect(await queries(page)).toBe(0);
  await openStorage(page);
  const dialog = page.locator('#attachment-storage-dialog');
  await expect(dialog).toHaveAttribute('aria-busy', 'false');
  expect(await queries(page)).toBe(1);
  await expect(page.locator('#attachment-storage-summary')).toContainText('전체 사용량');
  await expect(page.locator('#attachment-storage-summary')).toContainText('60 바이트');
  await expect(page.locator('#attachment-storage-summary')).toContainText('미전송 초안 · 현재 실행');
  await expect(page.locator('#attachment-storage-summary')).toContainText('보존 · 현재 실행');
  await expect(page.locator('#attachment-storage-summary')).toContainText('기타 · 이전 또는 다른 실행 포함');
  await expect(page.locator('#attachment-storage-limit')).toContainText('256 MiB');
  await expect(page.locator('#attachment-storage-drafts')).toContainText('draft-session');
  await expect(page.locator('#attachment-storage-guidance')).toContainText('해당 세션에서 첨부를 제거');
  await expect(dialog.locator('button')).toHaveCount(2);
  await page.locator('#attachment-storage-refresh').click();
  await expect.poll(() => queries(page)).toBe(2);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(page.locator('#attachment-storage-summary')).toBeEmpty();
  await expect(page.locator('#attachment-storage-drafts')).toBeEmpty();
  await expect(page.locator('#attachment-storage-open')).toBeFocused();
  expect(await page.evaluate(() => (window as any).storageCalls.filter((call: any) => /cleanup|delete|session_stop|complete_exit/.test(call.command)))).toEqual([]);
});

test('조회 중 새로고침을 중복하지 않고 닫힌 팝업의 지연 응답은 버린다', async ({ page }) => {
  await workspace(page, 'deferred'); await openStorage(page);
  await expect(page.locator('#attachment-storage-refresh')).toBeDisabled();
  await page.locator('#attachment-storage-refresh').evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
  expect(await queries(page)).toBe(1);
  await page.locator('#attachment-storage-close').click();
  await page.evaluate(() => { (window as any).finishStorage(); });
  await expect(page.locator('#attachment-storage-summary')).toBeEmpty();
  expect(await queries(page)).toBe(1);
  await page.evaluate(() => { (window as any).storageMode = 'normal'; });
  await page.locator('#attachment-storage-open').click();
  await expect(page.locator('#attachment-storage-summary')).toContainText('60 바이트');
  expect(await queries(page)).toBe(2);
});

test('저장소 조회 오류와 과대한 메타데이터를 성공한 빈 용량으로 표시하지 않는다', async ({ page }) => {
  await workspace(page, 'error'); await openStorage(page);
  await expect(page.locator('#attachment-storage-error')).toContainText('잠시 후 새로고침하세요.');
  await expect(page.locator('#attachment-storage-dialog')).not.toContainText('OS_ERROR_32');
  await expect(page.locator('#attachment-storage-dialog')).not.toContainText('D:\\private');
  await expect(page.locator('#attachment-storage-dialog')).not.toContainText('secret-token');
  await expect(page.locator('#attachment-storage-summary')).toBeEmpty();
  await page.evaluate(() => { (window as any).storageMode = 'oversized'; });
  await page.locator('#attachment-storage-refresh').click();
  await expect(page.locator('#attachment-storage-error')).toContainText('첨부 저장 공간을 확인하지 못했습니다.');
  await expect(page.locator('#attachment-storage-drafts li')).toHaveCount(0);
  await page.evaluate(() => { (window as any).storageMode = 'normal'; });
  await page.locator('#attachment-storage-refresh').click();
  await expect(page.locator('#attachment-storage-error')).toBeHidden();
  await expect(page.locator('#attachment-storage-summary')).toContainText('60 바이트');
});

test('조회가 끝나기 전에 다시 연 팝업은 이전 응답 대신 새 스냅샷을 표시한다', async ({ page }) => {
  await workspace(page, 'deferred'); await openStorage(page);
  await page.locator('#attachment-storage-close').click();
  await page.locator('#attachment-storage-open').click();
  expect(await queries(page)).toBe(1);
  await page.evaluate(() => {
    const state = window as any;
    state.storageMode = 'normal';
    // A stale malformed result must never become visible in the reopened popup.
    state.finishStorage({ totalBytes: -1, draftSessions: [] });
  });
  await expect.poll(() => queries(page)).toBe(2);
  await expect(page.locator('#attachment-storage-summary')).toContainText('60 바이트');
  await expect(page.locator('#attachment-storage-error')).toBeHidden();
});

test('영문 설정에서 저장 공간 분류와 보존 안내를 제공한다', async ({ page }) => {
  await workspace(page);
  await page.locator('#settings-open').click();
  await page.locator('#language-select').selectOption('en');
  await page.locator('#attachment-storage-open').click();
  const dialog = page.getByRole('dialog', { name: 'Attachment storage', exact: true });
  await expect(dialog).toBeVisible();
  await expect(page.locator('#attachment-storage-summary')).toContainText('Retained · current process');
  await expect(page.locator('#attachment-storage-summary')).toContainText('Other · includes previous or other processes');
  await expect(page.locator('#attachment-storage-preservation')).toContainText('uncertain result');
  await expect(page.locator('#attachment-storage-refresh')).toHaveText('Refresh');
  await page.getByRole('button', { name: 'Close attachment storage', exact: true }).click();
  await expect(dialog).toBeHidden();
});
