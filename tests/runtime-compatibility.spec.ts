import { test, expect, type Page } from '@playwright/test';

async function openSettings(page: Page, hold = false) {
  await page.addInitScript(({ hold }) => {
    const state = window as any; state.isTauri = true;
    state.compatibilityFixture = { calls: [], reply: { executable: 'D:\\tools\\codex.exe', cliVersion: 'codex-cli 0.2.0', serverState: 'running', serverVersion: '0.1.0', subagents: false, compactIfIdle: null, expectedTurnInterrupt: null }, release: () => {} };
    state.__TAURI_INTERNALS__ = { transformCallback: () => 1, unregisterCallback() {}, async invoke(command: string, args: any = {}) {
      const fixture = state.compatibilityFixture; fixture.calls.push({command,args});
      if(command.startsWith('plugin:event|')) return 1;
      if(command === 'get_ui_language') return 'ko';
      if(command === 'set_ui_language') return {};
      if(command === 'load_layout') return {version:1,columns:2,sessions:[]};
      if(command === 'list_projects' || command === 'list_sessions') return [];
      if(command === 'get_codex_settings') return {savedPath:null,resolvedPath:'D:\\tools\\codex.exe',source:'path',error:null};
      if(command === 'runtime_compatibility') { if(hold) await new Promise<void>(resolve => {fixture.release=resolve;}); return fixture.reply; }
      return null;
    }};
  }, {hold});
  await page.goto('/'); await expect(page.locator('#app')).toHaveAttribute('aria-busy','false');
  await page.locator('#settings-open').click(); await page.locator('#codex-settings-open').click();
}

test('호환성은 별도 버전과 서버가 알린 기능을 표시하고 조회 외 명령을 보내지 않는다', async ({page}) => {
  await openSettings(page);
  expect(await page.evaluate(() => (window as any).compatibilityFixture.calls.filter((c:any) => c.command==='runtime_compatibility').length)).toBe(0);
  await page.locator('.compatibility-refresh').click();
  await expect(page.locator('.compatibility-details')).toContainText('codex-cli 0.2.0');
  await expect(page.locator('.compatibility-details')).toContainText('0.1.0');
  await expect(page.locator('.compatibility-details dd')).toHaveText(['D:\\tools\\codex.exe','codex-cli 0.2.0','0.1.0','지원되지 않음','미확인','미확인']);
  await expect(page.locator('.compatibility-help')).toContainText('실행 중인 서버는 바뀌지');
  await page.locator('#codex-settings-close').click();
  await page.locator('#language-select').selectOption('en');
  await page.locator('#codex-settings-open').click(); await page.locator('.compatibility-refresh').click();
  await expect(page.locator('.compatibility-details')).toContainText('Not supported');
  const calls = await page.evaluate(() => (window as any).compatibilityFixture.calls);
  expect(calls.filter((c:any)=> /start_session|session_stop|set_codex|choose_codex|complete_exit/.test(c.command))).toEqual([]);
});

test('닫힌 화면의 지연 응답을 버리고 조회 실패를 지원됨으로 표시하지 않는다', async ({page}) => {
  await openSettings(page,true); await page.locator('.compatibility-refresh').click();
  await expect(page.locator('.compatibility-refresh')).toBeDisabled();
  await page.locator('#codex-settings-close').click(); await page.locator('#codex-settings-open').click();
  await page.evaluate(() => (window as any).compatibilityFixture.release());
  await expect(page.locator('.compatibility-refresh')).toBeEnabled();
  await expect(page.locator('.compatibility-details')).toBeHidden();
  await page.evaluate(() => { (window as any).compatibilityFixture.reply={executable:'fixture',cliVersion:null,serverState:'unavailable',serverVersion:null,subagents:null,compactIfIdle:null,expectedTurnInterrupt:null}; });
  await page.locator('.compatibility-refresh').click(); await page.evaluate(() => (window as any).compatibilityFixture.release());
  await expect(page.locator('.compatibility-status')).toContainText('공유서버 연결을 확인하지 못했습니다');
  await expect(page.locator('.compatibility-details dd')).toHaveText(['fixture','미확인','미확인','미확인','미확인','미확인']);
  await page.evaluate(() => { (window as any).compatibilityFixture.reply.subagents='true'; });
  await page.locator('.compatibility-refresh').click(); await page.evaluate(() => (window as any).compatibilityFixture.release());
  await expect(page.locator('.compatibility-status')).toContainText('호환성을 확인하지 못했습니다');
  await expect(page.locator('.compatibility-details')).toBeHidden();
});
