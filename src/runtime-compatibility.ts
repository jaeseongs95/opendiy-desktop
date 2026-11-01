import { onLanguageChange, t } from './i18n';

type Report = {
  executable: string; cliVersion: string | null; serverVersion: string | null;
  serverState: 'running' | 'unsupported' | 'unavailable';
  subagents: boolean | null; compactIfIdle: boolean | null; expectedTurnInterrupt: boolean | null;
};

export function createRuntimeCompatibility(call: <T>(command: string) => Promise<T>) {
  const element = document.createElement('section');
  element.className = 'runtime-compatibility';
  element.innerHTML = '<h3></h3><p class="compatibility-help"></p><button class="compatibility-refresh quiet-button" type="button"></button><p class="compatibility-status" role="status"></p><dl class="executable-details compatibility-details" hidden></dl><p class="compatibility-scope"></p>';
  const refresh = element.querySelector<HTMLButtonElement>('button')!;
  const status = element.querySelector<HTMLElement>('.compatibility-status')!;
  const details = element.querySelector<HTMLElement>('dl')!;
  let report: Report | undefined;
  let generation = 0;
  let pending: Promise<unknown> | undefined;
  let state: 'empty' | 'loading' | 'ready' | 'failed' = 'empty';
  const label = (value: boolean | null) => value === true ? t('지원됨', 'Supported') : value === false ? t('지원되지 않음', 'Not supported') : t('미확인', 'Unknown');
  function render() {
    element.querySelector('h3')!.textContent = t('실행 기반 호환성', 'Runtime compatibility');
    element.querySelector('.compatibility-help')!.textContent = t('선택한 실행 파일과 현재 공유서버를 따로 확인합니다. 파일을 바꿔도 실행 중인 서버는 바뀌지 않습니다.', 'Check the selected executable and current shared server separately. Choosing a file does not replace a running server.');
    element.querySelector('.compatibility-scope')!.textContent = t('기능 지원은 이미 열린 세션에서 확인한 같은 서버의 선언만 표시합니다. 확인되지 않은 기능은 미확인이며, 조회를 위해 세션을 재개하지 않습니다. 각 기능에서 연결·권한·작업 상태를 추가 확인합니다. 서버 갱신은 작업을 마친 뒤 별도로 진행하세요.', 'Support uses declarations already received by open sessions connected to the same server. Unobserved features remain unknown; checking does not resume sessions. Each feature also checks connection, permissions and task state. Update the server separately after ongoing work finishes.');
    refresh.textContent = t('호환성 확인', 'Check compatibility'); refresh.disabled = !!pending;
    status.textContent = state === 'loading' ? t('확인 중…', 'Checking…') : state === 'failed' ? t('호환성을 확인하지 못했습니다. 실행 파일과 서버 상태를 확인한 뒤 다시 조회하세요.', 'Could not check compatibility. Check the executable and server status, then retry.') : state === 'empty' ? t('호환성 확인을 눌러 현재 상태를 조회하세요.', 'Check compatibility to read the current state.') : report?.serverState === 'running' ? t('현재 공유서버의 실행 정보를 확인했습니다.', 'Read the current shared server status.') : report?.serverState === 'unsupported' ? t('선택한 실행 파일에 공유서버 조회 기능이 없습니다. 개별 세션의 지원 여부를 확인하세요.', 'The selected executable has no shared-server query support. Check support in each session.') : t('공유서버 연결을 확인하지 못했습니다. 서버 실행 상태를 확인한 뒤 다시 조회하세요.', 'Could not confirm the shared server connection. Check that the server is running, then retry.');
    details.replaceChildren(); details.hidden = !report;
    if (!report) return;
    for (const [title, value] of [
      [t('확인한 실행 파일', 'Checked executable'), report.executable],
      [t('실행 파일 버전', 'Executable version'), report.cliVersion ?? t('미확인', 'Unknown')],
      [t('현재 공유서버 버전', 'Current shared server version'), report.serverVersion ?? t('미확인', 'Unknown')],
      [t('서브에이전트 관리', 'Subagent management'), label(report.subagents)],
      [t('안전한 유휴 압축', 'Safe idle compaction'), label(report.compactIfIdle)],
      [t('지정 작업만 중단', 'Interrupt exact turn'), label(report.expectedTurnInterrupt)],
    ]) { const dt = document.createElement('dt'); dt.textContent = title; const dd = document.createElement('dd'); dd.textContent = value; details.append(dt, dd); }
  }
  refresh.onclick = () => {
    if (pending) return;
    const token = generation;
    report = undefined; state = 'loading';
    const operation = call<Report>('runtime_compatibility'); pending = operation; render();
    void operation.then(value => {
      if (generation !== token || !element.closest('dialog')?.open) return;
      if (!value || typeof value.executable !== 'string' || !['running','unsupported','unavailable'].includes(value.serverState)
        || [value.cliVersion,value.serverVersion].some(item => item !== null && (typeof item !== 'string' || item.length > 1024))
        || [value.subagents,value.compactIfIdle,value.expectedTurnInterrupt].some(item => item !== null && typeof item !== 'boolean')) throw new Error('Invalid compatibility response');
      report = value; state = 'ready';
    }).catch(() => { if (generation === token) state = 'failed'; })
      .finally(() => { if (pending === operation) pending = undefined; render(); });
  };
  onLanguageChange(render); render();
  return { element, invalidate() { generation++; report = undefined; state = 'empty'; render(); } };
}
