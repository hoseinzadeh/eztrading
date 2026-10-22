'use strict';
// Strategy Studio front end. Talks to the JSON API in core/studio/src/server.rs.

const MONACO_CDN = 'https://cdn.jsdelivr.net/npm/monaco-editor@0.50.0/min';
const $ = (id) => document.getElementById(id);
const PALETTE = ['#5aa2ff', '#e8914d', '#2fcf7a', '#c586c0', '#e6b422', '#4ec9b0', '#ff7a7a', '#9aa0ff', '#b5cea8', '#d7ba7d'];
const MUTED = '#9c9087';
const GRID = '#2a231f';
const Y_AXIS_WIDTH = 66;
const PLOT_PAD_RIGHT = 8;

const state = {
  file: null,
  dirty: false,
  loading: false,
  files: [],
  machine: null,
  analysis: null,
  compileDiags: [],
  simDiags: [],
  backtest: null,
  golden: null,
  testsFile: null,
  testsDirty: false,
  lastMermaid: '',
  charts: {},
  holders: {},
  ai: { enabled: false, source: null },
};

let editor = null;
let monacoRef = null;

// ---- small helpers ---------------------------------------------------------

function el(tag, props, ...children) {
  const node = document.createElement(tag);
  Object.assign(node, props || {});
  for (const c of children) node.append(c);
  return node;
}

async function api(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data = null;
  try { data = await res.json(); } catch { /* not JSON */ }
  if (!res.ok) throw new Error((data && data.error) || `${res.status} ${res.statusText}`);
  return data;
}

function say(message, kind) {
  const node = $('status');
  node.textContent = message || '';
  node.className = 'status ' + (kind || '');
  clearTimeout(say.timer);
  if (message && kind) say.timer = setTimeout(() => say(''), 7000);
}

function fmtNum(x, digits = 2) {
  if (x === null || x === undefined || Number.isNaN(x)) return '–';
  return x.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

function fmtJson(obj) {
  // Pretty-print, but keep arrays of plain values (price series) on one line.
  return JSON.stringify(obj, null, 2).replace(/\[\s+([^\[\]{}]*?)\s+\]/g, (_m, inner) => '[' + inner.replace(/\s*\n\s*/g, ' ') + ']');
}

function baseName(file) { return file.replace(/\.test\.json$/, '').replace(/\.sm$/, ''); }

// ---- editor ----------------------------------------------------------------

function bootEditor() {
  return new Promise((resolve) => {
    window.MonacoEnvironment = {
      getWorkerUrl: () => 'data:text/javascript;charset=utf-8,' + encodeURIComponent(
        `self.MonacoEnvironment={baseUrl:'${MONACO_CDN}/'};importScripts('${MONACO_CDN}/vs/base/worker/workerMain.js');`),
    };
    window.require.config({ paths: { vs: MONACO_CDN + '/vs' } });
    window.require(['vs/editor/editor.main'], () => {
      monacoRef = window.monaco;
      registerSml(monacoRef);
      editor = monacoRef.editor.create($('editor'), {
        value: '', language: 'sml', theme: 'ezt-dark', automaticLayout: true, minimap: { enabled: false },
        fontSize: 13, tabSize: 4, scrollBeyondLastLine: false, fixedOverflowWidgets: true,
      });
      editor.onDidChangeModelContent(onEdit);
      editor.addCommand(monacoRef.KeyMod.CtrlCmd | monacoRef.KeyCode.KeyS, () => saveFile());
      resolve();
    });
  });
}

let analyzeTimer = 0;
let analyzeSeq = 0;

function onEdit() {
  if (state.loading) return;
  setDirty(true);
  clearTimeout(analyzeTimer);
  analyzeTimer = setTimeout(analyze, 350);
}

function setDirty(flag) {
  state.dirty = flag;
  $('dirty').hidden = !flag;
}

async function analyze() {
  const seq = ++analyzeSeq;
  try {
    const r = await api('POST', '/api/analyze', { source: editor.getValue(), machine: state.machine });
    if (seq !== analyzeSeq) return; // a newer edit superseded this answer
    state.analysis = r;
    renderAnalysis();
  } catch (e) {
    say(e.message, 'err');
  }
}

// ---- problems and markers ----------------------------------------------------

function allProblems() {
  const a = (state.analysis && state.analysis.diagnostics) || [];
  const tag = (list, source) => list.map((d) => ({ ...d, source }));
  return [...tag(state.compileDiags, 'compiler'), ...tag(state.simDiags, 'simulator'), ...tag(a, 'analysis')];
}

function renderProblems() {
  const problems = allProblems();
  const list = $('problem-list');
  list.textContent = '';
  $('problem-count').textContent = String(problems.length);
  if (!problems.length) {
    list.append(el('li', { className: 'empty', textContent: 'No problems found.' }));
  }
  for (const d of problems) {
    const li = el('li', { title: d.source });
    li.append(
      el('span', { className: 'sev ' + d.severity, textContent: d.severity }),
      el('span', { className: 'pos', textContent: `${d.line}:${d.col}` }),
      el('span', { textContent: d.message }),
    );
    li.addEventListener('click', () => reveal(d.line, d.col));
    list.append(li);
  }
  const model = editor && editor.getModel();
  if (!model) return;
  const sev = { error: monacoRef.MarkerSeverity.Error, warning: monacoRef.MarkerSeverity.Warning, info: monacoRef.MarkerSeverity.Info };
  for (const owner of ['compiler', 'simulator', 'analysis']) {
    const markers = problems.filter((d) => d.source === owner).map((d) => markerFor(model, d, sev[d.severity]));
    monacoRef.editor.setModelMarkers(model, 'studio-' + owner, markers);
  }
}

function markerFor(model, d, severity) {
  const line = Math.min(Math.max(d.line, 1), model.getLineCount());
  const max = model.getLineMaxColumn(line);
  let start = d.col, end;
  if (d.col <= 1) {
    start = Math.max(1, model.getLineFirstNonWhitespaceColumn(line));
    end = max;
  } else {
    const word = model.getWordAtPosition({ lineNumber: line, column: d.col });
    end = word ? word.endColumn : Math.min(d.col + 1, max);
  }
  return { severity, message: d.message, startLineNumber: line, endLineNumber: line, startColumn: start, endColumn: Math.max(end, start + 1) };
}

function reveal(line, col) {
  editor.revealLineInCenter(line);
  editor.setPosition({ lineNumber: line, column: Math.max(col, 1) });
  editor.focus();
}

// ---- analysis: diagram and parameter form --------------------------------------

function renderAnalysis() {
  const a = state.analysis;
  renderProblems();
  renderMachineSelect(a.machines || []);
  renderParams(a.params || []);
  if (!$('deploy-modal').hidden) renderParams(a.params || [], 'dp-params');
  renderDiagram(a);
}

function renderMachineSelect(machines) {
  const select = $('machine-select');
  const wanted = state.machine && machines.includes(state.machine) ? state.machine : machines[0] || '';
  if (select.dataset.key !== machines.join('|')) {
    select.textContent = '';
    for (const m of machines) select.append(el('option', { value: m, textContent: m }));
    select.dataset.key = machines.join('|');
  }
  select.value = wanted;
  state.machine = wanted || null;
}

function renderParams(params, boxId = 'params') {
  const box = $(boxId);
  const previous = readParamInputs(boxId);
  const key = JSON.stringify(params.map((p) => [p.name, p.type, p.default]));
  if (box.dataset.key === key) return;
  box.dataset.key = key;
  box.textContent = '';
  if (!params.length) box.append(el('span', { className: 'muted', textContent: 'This machine has no parameters.' }));
  for (const p of params) {
    const label = el('label', { title: `${p.name}: ${p.type}` });
    label.append(document.createTextNode((p.label || p.name) + (p.required ? ' *' : '')));
    let input;
    if (p.options && p.options.length) {
      input = el('select');
      input.append(el('option', { value: '', textContent: '(default)' }));
      for (const o of p.options) input.append(el('option', { value: o.value, textContent: o.label || o.value }));
    } else {
      input = el('input', { placeholder: p.default !== null && p.default !== undefined ? String(p.default) : (p.hint || p.type) });
    }
    input.dataset.param = p.name;
    input.dataset.type = p.type;
    if (previous[p.name] !== undefined) input.value = previous[p.name];
    label.append(input);
    box.append(label);
  }
}

function readParamInputs(boxId = 'params') {
  const out = {};
  document.querySelectorAll(`#${boxId} [data-param]`).forEach((i) => { out[i.dataset.param] = i.value; });
  return out;
}

function readParams(boxId = 'params') {
  const out = {};
  document.querySelectorAll(`#${boxId} [data-param]`).forEach((i) => {
    const raw = i.value.trim();
    if (!raw) return;
    if (i.dataset.type.startsWith('Vec')) out[i.dataset.param] = raw.split(',').map((s) => s.trim()).filter(Boolean);
    else if (raw === 'true' || raw === 'false') out[i.dataset.param] = raw === 'true';
    else if (!Number.isNaN(Number(raw))) out[i.dataset.param] = Number(raw);
    else out[i.dataset.param] = raw; // e.g. "5 min", parsed by the simulator
  });
  return out;
}

let diagramSeq = 0;

async function renderDiagram(a) {
  const box = $('diagram');
  if (!a.ok) {
    if (!state.lastMermaid) box.textContent = 'Fix the syntax error to see the diagram.';
    box.title = 'showing the last valid version while the source has a syntax error';
    box.style.opacity = state.lastMermaid ? '0.45' : '1';
    return;
  }
  box.style.opacity = '1';
  box.title = '';
  const text = a.diagram && a.diagram.mermaid;
  if (!text) { box.textContent = 'No machine to draw.'; state.lastMermaid = ''; return; }
  if (text === state.lastMermaid) return;
  const mySeq = ++diagramSeq;
  try {
    const { svg } = await window.mermaid.render('mmd' + mySeq, text);
    if (mySeq !== diagramSeq) return;
    box.innerHTML = svg; // generated by Mermaid in strict mode from escaped labels
    state.lastMermaid = text;
  } catch (e) {
    box.textContent = 'Could not draw the diagram: ' + (e.message || e);
  }
}

// ---- files -------------------------------------------------------------------

async function loadFiles() {
  state.files = (await api('GET', '/api/files')).files;
  const list = $('file-list');
  list.textContent = '';
  for (const f of state.files) {
    const li = el('li', { className: f.name === state.file || f.name === state.testsFile ? 'active' : '' });
    li.append(el('span', { textContent: f.name }), el('span', { className: 'tag', textContent: f.kind === 'tests' ? 'tests' : '' }));
    li.addEventListener('click', () => openFile(f.name).catch((e) => say(e.message, 'err')));
    list.append(li);
  }
}

function confirmDiscard() {
  return !(state.dirty || state.testsDirty) || window.confirm('You have unsaved changes. Discard them?');
}

async function openFile(name) {
  if (!confirmDiscard()) return;
  const isTests = name.endsWith('.test.json');
  const strategy = isTests ? baseName(name) + '.sm' : name;
  if (strategy !== state.file && state.files.some((f) => f.name === strategy)) {
    const { content } = await api('GET', '/api/files/' + encodeURIComponent(strategy));
    state.loading = true;
    editor.setValue(content);
    state.loading = false;
    state.file = strategy;
    state.machine = null; // the machine list belongs to the file
    $('machine-select').dataset.key = '';
    $('file-name').textContent = strategy;
    setDirty(false);
    state.compileDiags = [];
    state.simDiags = [];
    state.lastMermaid = '';
    $('bt-results').hidden = true;
    $('bt-error').hidden = true;
    await analyze();
  }
  // The tests that belong to this strategy.
  const testsName = baseName(state.file || strategy) + '.test.json';
  if (state.files.some((f) => f.name === testsName)) {
    const { content } = await api('GET', '/api/files/' + encodeURIComponent(testsName));
    $('tests-text').value = content;
    state.testsFile = testsName;
  } else {
    $('tests-text').value = '';
    state.testsFile = null;
  }
  state.testsDirty = false;
  $('test-results').textContent = '';
  setTestsPill(null);
  if (isTests) switchTab('tests');
  await loadFiles();
}

async function saveFile() {
  if (!state.file) {
    if (editor) await createFile(editor.getValue());
    return;
  }
  try {
    if (state.dirty) {
      await api('PUT', '/api/files/' + encodeURIComponent(state.file), { content: editor.getValue() });
      setDirty(false);
    }
    const testsText = $('tests-text').value;
    if (state.testsDirty && testsText.trim()) {
      const name = baseName(state.file) + '.test.json';
      await api('PUT', '/api/files/' + encodeURIComponent(name), { content: testsText });
      state.testsFile = name;
      state.testsDirty = false;
    }
    await loadFiles();
    say('Saved', 'ok');
    if ($('run-on-save').checked && $('tests-text').value.trim()) runTests(false);
  } catch (e) {
    say(e.message, 'err');
  }
}

const NEW_TEMPLATE = `// New strategy
use std::*;

machine MyStrategy {
    param equities: Vec<Symbol> as "Equities";

    init { launch (watch(sym), WAIT) for sym in equities; }

    process watch(sym: Symbol) every 1 day {
        listen Tickers[sym];

        // Add states and goto transitions, e.g.
        //   state WAIT: goto BUY on Tickers[sym] < 10;
        state WAIT;
    }
}
`;

function askName() {
  return new Promise((resolve) => {
    const modal = $('modal');
    const input = $('modal-input');
    input.value = '';
    modal.hidden = false;
    input.focus();
    const done = (value) => {
      modal.hidden = true;
      $('modal-form').onsubmit = null;
      $('modal-cancel').onclick = null;
      resolve(value);
    };
    $('modal-form').onsubmit = (ev) => { ev.preventDefault(); done(input.value.trim()); };
    $('modal-cancel').onclick = () => done(null);
  });
}

async function createFile(content) {
  if (!confirmDiscard()) return;
  const raw = await askName();
  if (!raw) return;
  const name = raw.endsWith('.sm') ? raw : raw + '.sm';
  if (state.files.some((f) => f.name === name)) { say(`${name} already exists`, 'err'); return; }
  try {
    await api('PUT', '/api/files/' + encodeURIComponent(name), { content });
    await loadFiles();
    state.dirty = false;
    state.testsDirty = false;
    await openFile(name);
  } catch (e) {
    say(e.message, 'err');
  }
}

// ---- compile -----------------------------------------------------------------

async function compile() {
  say('Compiling…');
  try {
    const r = await api('POST', '/api/compile', { source: editor.getValue() });
    state.compileDiags = r.diagnostics;
    renderProblems();
    const analysisErrors = ((state.analysis && state.analysis.diagnostics) || []).filter((d) => d.severity === 'error').length;
    if (!r.ok) say('The compiler reported problems (see Problems).', 'err');
    else if (analysisErrors) say('The compiler accepted the syntax, but the analysis found errors it does not check (see Problems).', 'err');
    else say('The compiler accepted this strategy.', 'ok');
  } catch (e) {
    say(e.message, 'err');
  }
}

// ---- backtest ----------------------------------------------------------------

function switchTab(name) {
  document.querySelectorAll('.tab').forEach((t) => t.classList.toggle('active', t.dataset.tab === name));
  document.querySelectorAll('.panel').forEach((p) => p.classList.toggle('active', p.id === 'tab-' + name));
}

function backtestRequest() {
  const kind = $('data-kind').value;
  const symbol = $('data-symbol').value.trim() || 'TEST';
  const data = kind === 'generate'
    ? {
        kind, symbol,
        generator: {
          kind: $('gen-kind').value,
          bars: Number($('gen-bars').value),
          seed: Number($('gen-seed').value),
          start: Number($('gen-start').value),
          vol: Number($('gen-vol').value) / 100,
          period: Number($('gen-period').value),
        },
      }
    : { kind, symbol, csv: $('csv-text').value };
  return {
    source: editor.getValue(),
    machine: state.machine,
    params: readParams(),
    data,
    bar_seconds: Number($('bar-seconds').value),
    initial_cash: Number($('cash').value),
    fee_bps: Number($('fee').value),
  };
}

async function runBacktest() {
  switchTab('backtest');
  const msg = $('bt-msg');
  msg.textContent = 'running…';
  $('btn-run').disabled = $('btn-run2').disabled = true;
  state.simDiags = [];
  try {
    const req = backtestRequest();
    const r = await api('POST', '/api/backtest', req);
    if (!r.ok) {
      showBacktestError(r.error);
      return;
    }
    $('bt-error').hidden = true;
    state.backtest = r.backtest;
    state.golden = r.golden;
    renderBacktest(r.backtest);
    msg.textContent = `${r.backtest.stats.bars} bars, ${r.backtest.stats.fills} fills`;
  } catch (e) {
    showBacktestError({ message: e.message });
  } finally {
    $('btn-run').disabled = $('btn-run2').disabled = false;
    renderProblems();
  }
}

function showBacktestError(err) {
  $('bt-msg').textContent = '';
  $('bt-results').hidden = true;
  const box = $('bt-error');
  box.hidden = false;
  box.textContent = (err.line ? `Line ${err.line}: ` : '') + err.message + (err.bar !== undefined && err.bar !== null ? `  (bar ${err.bar})` : '');
  if (err.line) {
    state.simDiags = [{ line: err.line, col: 1, severity: 'error', message: err.message }];
    box.style.cursor = 'pointer';
    box.onclick = () => reveal(err.line, 1);
  } else {
    box.onclick = null;
    box.style.cursor = 'default';
  }
}

function statCard(label, value, cls, sub) {
  const card = el('div', { className: 'stat' });
  card.append(el('div', { className: 'k', textContent: label }), el('div', { className: 'v ' + (cls || ''), textContent: value }));
  if (sub) card.append(el('div', { className: 's', textContent: sub }));
  return card;
}

function renderBacktest(b) {
  $('bt-results').hidden = false;
  const s = b.stats;
  const sign = (x) => (x > 0 ? 'good' : x < 0 ? 'bad' : '');
  const stats = $('stats');
  stats.textContent = '';
  stats.append(
    statCard('Strategy return', fmtNum(s.total_return_pct) + '%', sign(s.total_return_pct)),
    statCard('Buy & hold', fmtNum(s.buy_hold_return_pct) + '%', sign(s.buy_hold_return_pct), b.symbol),
    statCard('Final equity', fmtNum(s.final_equity), '', 'from ' + fmtNum(s.initial_cash, 0)),
    statCard('Max drawdown', fmtNum(s.max_drawdown_pct) + '%', s.max_drawdown_pct > 0 ? 'bad' : ''),
    statCard('Fills', String(s.fills), '', s.closed_trades + ' closed'),
    statCard('Win rate', s.win_rate_pct === null ? '–' : fmtNum(s.win_rate_pct, 0) + '%'),
    statCard('Realized P&L', fmtNum(s.realized_pnl), sign(s.realized_pnl)),
    statCard('Time invested', fmtNum(s.exposure_pct, 0) + '%'),
  );

  const warn = $('warnings');
  warn.hidden = !b.warnings.length;
  warn.textContent = b.warnings.join('\n');

  showCharts('bt', { price: 'chart-price', equity: 'chart-equity', dd: 'chart-dd' }, backtestChartData(b));
  drawStrips($('strip'), b.timelines, b.bars.map((x) => x.label));

  const body = document.querySelector('#trades tbody');
  body.textContent = '';
  for (const t of b.trades.slice(0, 500)) {
    const tr = el('tr');
    const cells = [
      [String(t.bar)], [t.label], [t.side, t.side], [fmtNum(t.shares, 4)], [fmtNum(t.price, 4)], [t.process], [t.state],
      [t.pnl === null ? '' : fmtNum(t.pnl), t.pnl > 0 ? 'buy' : t.pnl < 0 ? 'sell' : ''],
    ];
    for (const [text, cls] of cells) tr.append(el('td', { textContent: text, className: cls || '' }));
    body.append(tr);
  }
  if (b.trades.length > 500) {
    const tr = el('tr');
    tr.append(el('td', { colSpan: 8, textContent: `… ${b.trades.length - 500} more` }));
    body.append(tr);
  }
  $('logs-wrap').hidden = !b.logs.length;
  $('logs').textContent = b.logs.slice(0, 200).join('\n');
}

// Charts are shared by the backtest view and the live dashboard. A "holder"
// keeps the labels the tick/tooltip callbacks read, so a chart can be updated in
// place (new bars appended) instead of being rebuilt every second.

function baseChartOptions(holder, title) {
  // ISO timestamps (live quote feeds) are shortened to `MM-DD HH:MM:SS` so the axis stays readable
  const short = (t) => { const m = /^\d{4}-(\d\d-\d\d)T(\d\d:\d\d(?::\d\d)?)/.exec(t); return m ? `${m[1]} ${m[2]}` : t; };
  const label = (v) => short(holder.labels[Math.round(v)] || '');
  return {
    parsing: false, normalized: true, animation: false, maintainAspectRatio: false,
    interaction: { mode: 'nearest', intersect: false, axis: 'x' },
    layout: { padding: { right: PLOT_PAD_RIGHT } },
    scales: {
      x: {
        type: 'linear', min: 0, max: 1,
        ticks: { callback: label, maxTicksLimit: 4, color: MUTED, align: 'inner', maxRotation: 0 },
        grid: { color: GRID },
      },
      y: {
        ticks: { color: MUTED }, grid: { color: GRID },
        afterFit: (scale) => { scale.width = Y_AXIS_WIDTH; },
      },
    },
    plugins: {
      title: { display: !!title, text: title, color: '#cfc6be', align: 'start', font: { weight: '600', size: 12 } },
      legend: { align: 'end', labels: { color: MUTED, boxWidth: 12 } },
      decimation: { enabled: true, algorithm: 'lttb', samples: 700 },
    },
  };
}

function destroyCharts(prefix) {
  for (const name of ['price', 'equity', 'dd']) {
    const key = prefix + name;
    if (state.charts[key]) { state.charts[key].destroy(); delete state.charts[key]; }
  }
  delete state.holders[prefix];
}

function createCharts(prefix, ids, data) {
  destroyCharts(prefix);
  const holder = { labels: data.labels };
  state.holders[prefix] = holder;
  const title = (c) => holder.labels[Math.round(c[0].parsed.x)] || '';
  const mk = (name, canvasId, config) => { state.charts[prefix + name] = new window.Chart($(canvasId), config); };

  const priceOpts = baseChartOptions(holder, 'Price · ' + data.symbol);
  priceOpts.plugins.tooltip = {
    callbacks: {
      title,
      label: (c) => {
        const t = c.raw && c.raw.t;
        return t ? `${t.side} ${fmtNum(t.shares, 4)} @ ${fmtNum(t.price, 4)}` + (t.pnl !== null ? `  P&L ${fmtNum(t.pnl)}` : '')
                 : `${c.dataset.label}: ${fmtNum(c.parsed.y, 4)}`;
      },
    },
  };
  mk('price', ids.price, {
    type: 'line',
    data: {
      datasets: [
        { label: 'Close', data: [], borderColor: '#e8914d', borderWidth: 1.4, pointRadius: 0, order: 3 },
        { label: 'Buy', type: 'scatter', data: [], pointStyle: 'triangle', pointRadius: 7, backgroundColor: '#2fcf7a', borderColor: '#0b3a22', order: 1 },
        { label: 'Sell', type: 'scatter', data: [], pointStyle: 'triangle', rotation: 180, pointRadius: 7, backgroundColor: '#ff5c5c', borderColor: '#4a1010', order: 2 },
      ],
    },
    options: priceOpts,
  });

  const eqOpts = baseChartOptions(holder, 'Equity');
  eqOpts.plugins.tooltip = { callbacks: { title, label: (c) => `${c.dataset.label}: ${fmtNum(c.parsed.y)}` } };
  mk('equity', ids.equity, {
    type: 'line',
    data: {
      datasets: [
        { label: 'Strategy', data: [], borderColor: '#2fcf7a', borderWidth: 1.6, pointRadius: 0 },
        { label: 'Buy & hold', data: [], borderColor: '#8a8178', borderDash: [5, 4], borderWidth: 1.2, pointRadius: 0 },
      ],
    },
    options: eqOpts,
  });

  const ddOpts = baseChartOptions(holder, 'Drawdown (%)');
  ddOpts.scales.y.max = 0;
  ddOpts.plugins.legend = { display: false };
  ddOpts.plugins.tooltip = { callbacks: { title, label: (c) => `drawdown ${fmtNum(c.parsed.y)}%` } };
  mk('dd', ids.dd, {
    type: 'line',
    data: { datasets: [{ label: 'Drawdown', data: [], borderColor: '#ff5c5c', backgroundColor: 'rgba(255,92,92,.22)', fill: 'origin', borderWidth: 1, pointRadius: 0 }] },
    options: ddOpts,
  });
}

/**
 * data: { symbol, labels, close, equity, buyHold, drawdown, trades, offset }
 * `offset` is the absolute bar index of labels[0] (trades carry absolute bars).
 */
function showCharts(prefix, ids, data) {
  const existing = state.charts[prefix + 'price'];
  if (!existing || existing.canvas !== $(ids.price) || state.holders[prefix].symbol !== data.symbol) createCharts(prefix, ids, data);
  const holder = state.holders[prefix];
  holder.labels = data.labels;
  holder.symbol = data.symbol;
  const xy = (values) => values.map((y, x) => ({ x, y }));
  const pt = (t) => ({ x: t.bar - data.offset, y: t.price, t });
  const visible = data.trades.filter((t) => t.bar >= data.offset);
  const xmax = Math.max(data.labels.length - 1, 1);

  const price = state.charts[prefix + 'price'];
  price.data.datasets[0].data = xy(data.close);
  price.data.datasets[1].data = visible.filter((t) => t.side === 'buy').map(pt);
  price.data.datasets[2].data = visible.filter((t) => t.side === 'sell').map(pt);
  const eq = state.charts[prefix + 'equity'];
  eq.data.datasets[0].data = xy(data.equity);
  eq.data.datasets[1].data = xy(data.buyHold);
  const dd = state.charts[prefix + 'dd'];
  dd.data.datasets[0].data = xy(data.drawdown);
  for (const c of [price, eq, dd]) {
    c.options.scales.x.max = xmax;
    c.update('none');
  }
}

function backtestChartData(b) {
  return {
    symbol: b.symbol, offset: 0, labels: b.bars.map((x) => x.label), close: b.bars.map((x) => x.close),
    equity: b.equity, buyHold: b.buy_hold, drawdown: b.drawdown_pct, trades: b.trades,
  };
}

const stateColors = new Map();
function colorFor(name) {
  if (!stateColors.has(name)) stateColors.set(name, PALETTE[stateColors.size % PALETTE.length]);
  return stateColors.get(name);
}

const stripObservers = new Map();

/** A lane per process showing which state it was in on each bar, aligned with the charts' plot area. */
function drawStrips(box, timelines, labels) {
  box.textContent = '';
  const n = labels.length;
  if (!n) return;
  const lanes = timelines.map((tl) => {
    const wrap = el('div');
    wrap.append(el('div', { className: 'muted', style: 'font-size:10.5px;margin-left:' + Y_AXIS_WIDTH + 'px', textContent: tl.process }));
    const canvas = el('canvas');
    wrap.append(canvas);
    box.append(wrap);
    return { tl, canvas };
  });
  const legend = el('div', { className: 'strip-legend', style: 'margin-left:' + Y_AXIS_WIDTH + 'px' });
  const seen = new Set();
  for (const { tl } of lanes) {
    for (const name of tl.states) {
      if (seen.has(name)) continue;
      seen.add(name);
      const item = el('span');
      item.append(el('i', { style: 'background:' + colorFor(name) }), document.createTextNode(name));
      legend.append(item);
    }
  }
  box.append(el('div', { className: 'muted', style: 'font-size:11px;margin:4px 0 0 ' + Y_AXIS_WIDTH + 'px', textContent: 'State each process was in when it woke up on each bar' }), legend);

  const draw = () => {
    const w = Math.max(20, box.clientWidth - Y_AXIS_WIDTH - PLOT_PAD_RIGHT);
    const dpr = window.devicePixelRatio || 1;
    for (const { tl, canvas } of lanes) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(15 * dpr);
      canvas.style.width = w + 'px';
      canvas.style.height = '15px';
      canvas.style.marginLeft = Y_AXIS_WIDTH + 'px';
      const ctx = canvas.getContext('2d');
      ctx.scale(dpr, dpr);
      const x = (i) => (n > 1 ? (i / (n - 1)) * w : 0);
      let start = 0;
      for (let i = 1; i <= n; i++) {
        if (i === n || tl.series[i] !== tl.series[start]) {
          ctx.fillStyle = colorFor(tl.states[tl.series[start]]);
          const x0 = x(start);
          const x1 = i === n ? w : x(i);
          ctx.fillRect(x0, 0, Math.max(1, x1 - x0), 15);
          start = i;
        }
      }
      canvas.onmousemove = (ev) => {
        const idx = Math.round((ev.offsetX / w) * (n - 1));
        const bar = Math.min(Math.max(idx, 0), n - 1);
        canvas.title = `${labels[bar]}: ${tl.states[tl.series[bar]]}`;
      };
    }
  };
  draw();
  const old = stripObservers.get(box.id);
  if (old) old.disconnect();
  const observer = new ResizeObserver(draw);
  observer.observe(box);
  stripObservers.set(box.id, observer);
}

// ---- tests -------------------------------------------------------------------

function setTestsPill(summary) {
  const pill = $('tests-pill');
  if (!summary) { pill.hidden = true; return; }
  pill.hidden = false;
  pill.textContent = `${summary.passed}/${summary.passed + summary.failed}`;
  pill.className = 'pill ' + (summary.failed ? 'bad' : 'good');
}

async function runTests(focus = true) {
  if (focus) switchTab('tests');
  const box = $('test-error');
  const list = $('test-results');
  try {
    const r = await api('POST', '/api/test', { source: editor.getValue(), tests: $('tests-text').value });
    list.textContent = '';
    if (!r.ok) {
      box.hidden = false;
      box.textContent = r.error;
      setTestsPill(null);
      return;
    }
    box.hidden = true;
    setTestsPill(r);
    for (const t of r.results) {
      const li = el('li', { className: t.passed ? 'pass' : 'fail' });
      li.append(el('div', { className: 'name', textContent: (t.passed ? '✔ ' : '✘ ') + t.name }));
      const detail = t.error ? 'Could not run: ' + t.error : t.failures.join('\n');
      if (detail) li.append(el('div', { className: 'detail', textContent: detail }));
      list.append(li);
    }
    say(`${r.passed} passed, ${r.failed} failed`, r.failed ? 'err' : 'ok');
  } catch (e) {
    say(e.message, 'err');
  }
}

function addGoldenTest() {
  if (!state.golden) return;
  let doc;
  const text = $('tests-text').value.trim();
  try {
    doc = text ? JSON.parse(text) : { tests: [] };
    if (!Array.isArray(doc.tests)) throw new Error('"tests" must be an array');
  } catch (e) {
    say('The test file is not valid JSON: ' + e.message, 'err');
    switchTab('tests');
    return;
  }
  const test = { ...state.golden, name: `captured from backtest #${doc.tests.length + 1}` };
  doc.tests.push(test);
  $('tests-text').value = fmtJson(doc);
  state.testsDirty = true;
  switchTab('tests');
  say('Test added. Save to keep it.', 'ok');
}

const TEST_TEMPLATE = {
  tests: [{
    name: 'describe the behaviour you expect',
    prices: [100, 98, 95, 99, 104, 110],
    params: {},
    expect: { min_trades: 1 },
  }],
};

// ---- AI ----------------------------------------------------------------------

async function loadAiStatus() {
  try {
    const s = await api('GET', '/api/ai/status');
    state.ai.enabled = s.enabled;
    $('ai-status').textContent = s.enabled ? `Connected: model ${s.model}. Requests use your API key and cost money.` : (s.note || 'AI is not available.');
    for (const id of ['ai-generate', 'ai-fix', 'ai-explain']) $(id).disabled = !s.enabled;
  } catch (e) {
    $('ai-status').textContent = e.message;
  }
}

async function aiRun(task) {
  const body = { prompt: $('ai-prompt').value, source: editor.getValue() };
  const busy = $('ai-busy');
  const buttons = ['ai-generate', 'ai-fix', 'ai-explain'].map($);
  buttons.forEach((b) => { b.disabled = true; });
  $('ai-error').hidden = true;
  const started = Date.now();
  const timer = setInterval(() => { busy.textContent = `working… ${Math.round((Date.now() - started) / 1000)}s`; }, 500);
  try {
    const r = await api('POST', '/api/ai/' + task, body);
    if (!r.ok) {
      $('ai-result').hidden = true;
      $('ai-error').hidden = false;
      $('ai-error').textContent = r.error;
      return;
    }
    state.ai.source = r.source;
    $('ai-result').hidden = false;
    $('ai-summary').textContent = r.summary || '(no summary)';
    $('ai-apply-row').hidden = !r.source;
    $('ai-usage').textContent = `${r.usage.turns} turn(s), ${r.usage.checks} compiler check(s), ${r.usage.input_tokens} input / ${r.usage.output_tokens} output tokens`;
  } catch (e) {
    $('ai-error').hidden = false;
    $('ai-error').textContent = e.message;
  } finally {
    clearInterval(timer);
    busy.textContent = '';
    buttons.forEach((b) => { b.disabled = !state.ai.enabled; });
  }
}

// ---- live deployments -----------------------------------------------------------

const live = { view: 'editor', list: [], meta: null, selected: null, cur: null, last: null, timer: 0, filter: 'all', brokers: { list: [], kill: false, allow_live: false }, brokerTick: 0 };
const LIVE_MAX_POINTS = 20000;
const MAX_ACTIVITY = 600;

function setView(view) {
  live.view = view;
  $('view-editor').classList.toggle('active', view === 'editor');
  $('view-live').classList.toggle('active', view === 'live');
  document.querySelector('.layout').hidden = view !== 'editor';
  document.querySelector('.file-actions').hidden = view !== 'editor';
  document.querySelector('.run-actions').hidden = view !== 'editor';
  $('live-view').hidden = view !== 'live';
  $('cluster-view').hidden = view !== 'cluster';
  $('view-cluster').classList.toggle('active', view === 'cluster');
  clearTimeout(live.timer);
  clearTimeout(cluster.timer);
  if (view === 'live') liveLoop(true);
  if (view === 'cluster') clusterLoop(true);
}

// ---- server cluster (srvman) ------------------------------------------------------------
const cluster = { configured: false, data: null, timer: 0, task: null };

function clusterLoop(immediate) {
  clearTimeout(cluster.timer);
  const run = async () => {
    if (live.view !== 'cluster') return;
    if (!document.hidden) { try { await refreshCluster(); } catch (e) { $('cluster-offline').hidden = false; $('cluster-offline').textContent = e.message; } }
    cluster.timer = setTimeout(run, 3000);
  };
  cluster.timer = setTimeout(run, immediate ? 0 : 3000);
}

async function initCluster() {
  try {
    const d = await api('GET', '/api/cluster');
    cluster.configured = !!d.configured;
  } catch { cluster.configured = false; }
  $('view-cluster').hidden = !cluster.configured;
  $('btn-cluster-deploy').hidden = !cluster.configured;
}

async function refreshCluster() {
  const d = await api('GET', '/api/cluster');
  cluster.data = d;
  const off = $('cluster-offline');
  off.hidden = d.connected !== false && !(d.problems && d.problems.length);
  off.textContent = d.connected === false ? `Cannot reach srvman at ${d.addr}: ${d.error}` : (d.problems || []).join('\n');
  $('cluster-addr').textContent = d.addr || '';
  const servers = $('cluster-servers');
  servers.textContent = '';
  for (const s of d.servers || []) {
    const down = /down$/.test(s);
    servers.append(el('li', { className: down ? 'down' : 'up', textContent: (down ? '✖ ' : '● ') + s }));
  }
  const bots = document.querySelector('#cluster-bots tbody');
  bots.textContent = '';
  const users = new Map((d.users || []).map((u) => [u.id, `${u.name} ${u.lastname || ''}`.trim()]));
  for (const b of d.bots || []) {
    const tr = el('tr');
    const stop = el('button', { className: 'btn danger', textContent: 'Stop' });
    stop.addEventListener('click', async () => {
      const ok = await choose({ title: `Stop bot ${b.id}?`, text: 'The bot stops and is removed from the database.', options: [{ id: 'yes', label: 'Stop it', cls: 'danger' }] });
      if (!ok) return;
      try { await api('POST', `/api/cluster/bots/${b.id}/stop`, {}); say(`Stopped bot ${b.id}`, 'ok'); } catch (e) { say(e.message, 'err'); }
      refreshCluster().catch(() => {});
    });
    const act = el('td'); act.append(stop);
    tr.append(el('td', { textContent: String(b.id) }), el('td', { textContent: b.plugin_name }), el('td', { textContent: b.machine.replace(/^.*::/, '') }), el('td', { textContent: users.get(b.user_id) || String(b.user_id) }), el('td', { textContent: b.server == null ? '–' : String(b.server) }), act);
    bots.append(tr);
  }
  if (!(d.bots || []).length) { const tr = el('tr'); tr.append(el('td', { colSpan: 6, className: 'muted', textContent: 'No bots are running.' })); bots.append(tr); }
  const plugins = document.querySelector('#cluster-plugins tbody');
  plugins.textContent = '';
  for (const p of d.plugins || []) {
    const tr = el('tr');
    tr.append(el('td', { textContent: String(p.id) }), el('td', { textContent: p.name }), el('td', { textContent: p.owner }), el('td', { textContent: p.machines.map((m) => m.name).join(', ') }), el('td', { textContent: p.version }));
    plugins.append(tr);
  }
  if (!(d.plugins || []).length) { const tr = el('tr'); tr.append(el('td', { colSpan: 5, className: 'muted', textContent: 'No plugins yet.' })); plugins.append(tr); }
  const tasks = await api('GET', '/api/cluster/tasks');
  const tl = $('cluster-tasks');
  tl.textContent = '';
  for (const t of tasks.tasks) {
    const li = el('li');
    li.append(el('div', { className: 'bn', textContent: t.name + (t.done ? (t.error ? ' — failed' : ' — done') : ' — running…') }));
    for (const s of t.steps) li.append(el('div', { className: 'bd step-' + s.status, textContent: `${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✖' : '…'} ${s.label}${s.detail ? ': ' + s.detail : ''}` }));
    tl.append(li);
  }
  if (!tasks.tasks.length) tl.append(el('li', { className: 'muted', textContent: 'Nothing deployed from here yet.' }));
  const n = (d.bots || []).length;
  $('cluster-pill').hidden = !n;
  $('cluster-pill').textContent = String(n);
  $('cluster-pill').className = 'pill run';
}

async function openClusterDeploy() {
  if (!editor) return;
  const a = state.analysis;
  if (!a || !a.ok) { say('Fix the syntax error before deploying.', 'err'); return; }
  const errors = (a.diagnostics || []).filter((d) => d.severity === 'error');
  if (errors.length) { say(`Fix ${errors.length} error(s) before deploying: ${errors[0].message}`, 'err'); return; }
  let d;
  try { d = await api('GET', '/api/cluster'); } catch (e) { say(e.message, 'err'); return; }
  if (d.connected === false) { say(`Cannot reach srvman at ${d.addr}: ${d.error}`, 'err'); return; }
  cluster.data = d;
  const base = (state.file ? baseName(state.file) : 'strategy').replace(/[^A-Za-z0-9_-]/g, '_');
  $('cl-name').value = base;
  $('cl-source').textContent = `${state.file || 'unsaved buffer'}${state.dirty ? ' (includes unsaved changes)' : ''}`;
  const ms = $('cl-machine');
  ms.textContent = '';
  for (const m of a.machines || []) ms.append(el('option', { value: m, textContent: m }));
  if (state.machine) ms.value = state.machine;
  const us = $('cl-user');
  us.textContent = '';
  for (const u of d.users || []) us.append(el('option', { value: u.id, textContent: `${u.name} ${u.lastname || ''} (#${u.id})` }));
  renderParams(a.params || [], 'cl-params');
  $('cl-steps').textContent = '';
  $('cl-error').hidden = true;
  $('cl-submit').disabled = false;
  $('cluster-modal').hidden = false;
  $('cl-name').focus();
}

async function submitClusterDeploy(ev) {
  ev.preventDefault();
  const params = {};
  for (const [k, v] of Object.entries(readParams('cl-params'))) params[k] = Array.isArray(v) ? v.join(',') : String(v);
  $('cl-submit').disabled = true;
  $('cl-error').hidden = true;
  const steps = $('cl-steps');
  steps.textContent = '';
  try {
    const { task } = await api('POST', '/api/cluster/deploy', { user: Number($('cl-user').value), name: $('cl-name').value.trim(), source: editor.getValue(), machine: $('cl-machine').value, params });
    for (;;) {
      const t = await api('GET', `/api/cluster/tasks/${task}`);
      steps.textContent = '';
      for (const s of t.steps) {
        const li = el('li');
        li.append(el('b', { className: s.status === 'ok' ? 'ok' : s.status === 'failed' ? 'no' : '', textContent: s.status === 'ok' ? '✓' : s.status === 'failed' ? '✖' : '…' }), el('span', { textContent: `${s.label}${s.detail ? ': ' + s.detail : ''}` }));
        steps.append(li);
      }
      if (t.done) {
        if (t.error) { $('cl-error').hidden = false; $('cl-error').textContent = t.error; }
        else say(`Bot ${t.bot ?? ''} is running on the cluster`, 'ok');
        break;
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
  } catch (e) {
    $('cl-error').hidden = false;
    $('cl-error').textContent = e.message;
  } finally {
    $('cl-submit').disabled = false;
  }
}

function setOffline(flag) { $('live-offline').hidden = !flag; }

function liveLoop(immediate) {
  clearTimeout(live.timer);
  const run = async () => {
    if (live.view !== 'live') return;
    if (!document.hidden) {
      try { await liveRefresh(); setOffline(false); } catch (e) { setOffline(true); }
    }
    live.timer = setTimeout(run, 1000);
  };
  live.timer = setTimeout(run, immediate ? 0 : 1000);
}

async function liveRefresh() {
  await refreshList();
  if (live.selected) await refreshDetail();
}

function fmtMoney(x) { return fmtNum(x, Math.abs(x) >= 1000 ? 0 : 2); }

function fmtDuration(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${String(m).padStart(2, '0')}m`;
  if (m) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${s}s`;
}

function ago(ms) { return ms == null ? 'never' : (Date.now() - ms < 2000 ? 'just now' : fmtDuration(Date.now() - ms) + ' ago'); }

function badge(status) {
  const b = el('span', { className: 'badge ' + status });
  b.append(el('span', { className: 'dot' }), document.createTextNode(status));
  return b;
}

function sparkline(values, good) {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 96 28');
  if (values.length > 1) {
    const lo = Math.min(...values), hi = Math.max(...values), span = hi - lo || 1;
    const pts = values.map((v, i) => `${(i / (values.length - 1)) * 96},${26 - ((v - lo) / span) * 24}`).join(' ');
    const line = document.createElementNS(ns, 'polyline');
    line.setAttribute('points', pts);
    line.setAttribute('fill', 'none');
    line.setAttribute('stroke', good ? '#2fcf7a' : '#ff5c5c');
    line.setAttribute('stroke-width', '1.5');
    svg.append(line);
  }
  return svg;
}

async function refreshBrokers() {
  const r = await api('GET', '/api/brokers');
  live.brokers = { list: r.brokers, kill: r.kill_switch, allow_live: r.allow_live };
  renderBrokers();
}

function envPill(env) {
  return el('span', { className: 'env ' + (env || 'simulated'), textContent: env || 'invalid' });
}

function renderBrokers() {
  const b = live.brokers;
  const kt = $('kill-toggle');
  kt.textContent = 'Kill switch: ' + (b.kill ? 'ON' : 'off');
  kt.classList.toggle('armed', b.kill);
  $('emergency').hidden = !b.list.length;
  kt.hidden = !b.list.length;
  const ul = $('broker-list');
  ul.textContent = '';
  for (const c of b.list) {
    const li = el('li');
    const head = el('div', { className: 'bn' });
    head.append(el('span', { textContent: c.name }), envPill(c.environment));
    const test = el('button', { className: 'btn', textContent: 'Test' });
    test.addEventListener('click', () => openBrokerTest(c.name));
    head.append(test);
    li.append(head);
    if (c.description) li.append(el('div', { className: 'bd', textContent: c.description }));
    if (c.host) li.append(el('div', { className: 'bd', textContent: c.host + (c.limits && c.limits.max_order_value ? ` · max order ${fmtMoney(c.limits.max_order_value)}` : '') }));
    if (c.error) li.append(el('div', { className: 'bw', textContent: c.error }));
    if (c.missing_env && c.missing_env.length) li.append(el('div', { className: 'bw', textContent: 'Set in the server\'s environment: ' + c.missing_env.join(', ') }));
    ul.append(li);
  }
  if (!b.list.length) ul.append(el('li', { className: 'muted', textContent: 'None. Add a description file under brokers/ in your workspace.' }));
}

async function toggleKill() {
  try {
    await api('POST', '/api/brokers/kill', { on: !live.brokers.kill });
    await refreshBrokers();
  } catch (e) { say(e.message, 'err'); }
}

async function emergencyStop() {
  const ok = await choose({
    title: 'Emergency stop?',
    text: 'Turns the kill switch on, pauses every deployment that trades through a brokerage and cancels their working orders. Open positions are left as they are (use Flatten per deployment to close them).',
    options: [{ id: 'yes', label: 'Stop everything', cls: 'danger' }],
  });
  if (!ok) return;
  try {
    const r = await api('POST', '/api/live/emergency-stop', {});
    say(`Emergency stop: ${r.paused.length} deployment(s) paused`, 'ok');
  } catch (e) { say(e.message, 'err'); }
  await liveRefresh().catch(() => {});
  await refreshBrokers().catch(() => {});
}

let testingBroker = null;
function openBrokerTest(name) {
  testingBroker = name;
  $('bt-title').textContent = 'Test ' + name;
  $('bt-steps').textContent = '';
  $('bt-modal').hidden = false;
}

async function runBrokerTest() {
  const ul = $('bt-steps');
  ul.textContent = '';
  ul.append(el('li', { className: 'muted', textContent: 'Contacting the brokerage…' }));
  $('bt-run').disabled = true;
  try {
    const r = await api('POST', `/api/brokers/${encodeURIComponent(testingBroker)}/test`, { symbol: $('bt-symbol').value.trim() || null });
    ul.textContent = '';
    for (const st of r.steps) {
      const li = el('li');
      li.append(el('b', { className: st.ok ? 'ok' : 'no', textContent: st.ok ? '✓' : '✖' }), el('span', { textContent: `${st.name}: ${st.detail}` }), el('span', { className: 'ms', textContent: st.ms + ' ms' }));
      ul.append(li);
    }
  } catch (e) {
    ul.textContent = '';
    ul.append(el('li', { className: 'no', textContent: e.message }));
  } finally {
    $('bt-run').disabled = false;
  }
}

async function refreshList() {
  const r = await api('GET', '/api/live');
  live.list = r.deployments;
  live.meta = r;
  if (live.brokerTick++ % 5 === 0 || live.brokers.kill !== r.kill_switch) refreshBrokers().catch(() => {});
  if (live.selected && !live.list.some((d) => d.id === live.selected)) {
    live.selected = null;
    live.cur = null;
    showDetail(false);
  }
  renderList();
  updateLivePill();
}

function updateLivePill() {
  const running = live.list.filter((d) => d.status === 'running').length;
  const pill = $('live-pill');
  pill.hidden = !running;
  pill.textContent = String(running);
  pill.className = 'pill run';
}

function renderList() {
  const list = $('dep-list');
  list.textContent = '';
  const running = live.list.filter((d) => d.status === 'running').length;
  const pnl = live.list.reduce((sum, d) => sum + (d.equity - d.initial_cash), 0);
  const sum = $('live-summary');
  sum.textContent = '';
  const pnlSpan = el('b', { textContent: (pnl >= 0 ? '+' : '') + fmtMoney(pnl) });
  pnlSpan.style.color = pnl > 0 ? 'var(--good)' : pnl < 0 ? 'var(--bad)' : '';
  sum.append(el('b', { textContent: String(live.list.length) }), document.createTextNode(` deployment${live.list.length === 1 ? '' : 's'} · `),
    el('b', { textContent: String(running) }), document.createTextNode(' running · total P&L '), pnlSpan);
  for (const d of live.list) {
    const card = el('li', { className: 'dep-card' + (d.id === live.selected ? ' active' : '') });
    const top = el('div', { className: 'top' });
    top.append(el('span', { className: 'name', textContent: d.name }), badge(d.status));
    const row = el('div', { className: 'row2' });
    const left = el('div');
    left.append(
      el('div', { className: 'ret ' + (d.return_pct > 0 ? 'good' : d.return_pct < 0 ? 'bad' : ''), textContent: (d.return_pct >= 0 ? '+' : '') + fmtNum(d.return_pct) + '%' }),
      el('div', { className: 'eq', textContent: `${fmtMoney(d.equity)} · ${d.fills} fills` }),
    );
    row.append(left, sparkline(d.spark, d.return_pct >= 0));
    const feedLine = el('div', { className: 'feed' });
    feedLine.append(document.createTextNode(d.feed + ' · '), d.execution === 'simulated' ? el('span', { textContent: 'simulated account' }) : envPill(d.environment), document.createTextNode(d.execution === 'simulated' ? '' : ' ' + d.execution));
    card.append(top, feedLine, row,
      el('div', { className: 'states', textContent: d.processes.map((p) => `${p.name}: ${p.state}`).join(' · ') }));
    card.addEventListener('click', () => selectDeployment(d.id));
    list.append(card);
  }
  if (!live.list.length) list.append(el('li', { className: 'muted', textContent: 'No deployments yet.' }));
}

function showDetail(flag) {
  $('live-detail').hidden = !flag;
  $('live-empty').hidden = flag;
}

async function selectDeployment(id) {
  live.selected = id;
  live.cur = null;
  live.last = null;
  destroyCharts('lv');
  $('activity').textContent = '';
  renderList();
  showDetail(true);
  try { await refreshDetail(); } catch (e) { say(e.message, 'err'); }
}

function initCur(d) {
  return {
    id: d.summary.id,
    offset: d.series.from,
    labels: d.series.labels.slice(),
    close: d.series.close.slice(),
    equity: d.series.equity.slice(),
    drawdown: d.series.drawdown_pct.slice(),
    position: d.series.position.slice(),
    timelines: d.series.timelines.map((t) => ({ ...t, series: t.series.slice() })),
    trades: d.trades.slice(),
    events: d.events.slice(),
    lastSeq: d.last_seq,
    firstClose: d.first_close,
  };
}

async function refreshDetail() {
  const id = live.selected;
  const cur = live.cur;
  let url = '/api/live/' + encodeURIComponent(id);
  if (cur && cur.id === id) url += `?bars_from=${cur.offset + cur.labels.length}&trades_from=${cur.trades.length}&after_seq=${cur.lastSeq}`;
  let d;
  try { d = await api('GET', url); } catch (e) {
    if (/no such deployment/.test(e.message)) { live.selected = null; live.cur = null; showDetail(false); return; }
    throw e;
  }
  if (live.selected !== id) return; // the user picked another one meanwhile
  let first = false, newBars = 0, newEvents = 0, newTrades = 0;
  if (!cur || cur.id !== id) {
    live.cur = initCur(d);
    first = true;
  } else {
    const expected = cur.offset + cur.labels.length;
    if (d.series.from !== expected || cur.labels.length > LIVE_MAX_POINTS) { live.cur = null; return refreshDetail(); }
    newBars = d.series.labels.length;
    cur.labels.push(...d.series.labels);
    cur.close.push(...d.series.close);
    cur.equity.push(...d.series.equity);
    cur.drawdown.push(...d.series.drawdown_pct);
    cur.position.push(...d.series.position);
    d.series.timelines.forEach((t, i) => cur.timelines[i].series.push(...t.series));
    newTrades = d.trades.length;
    cur.trades.push(...d.trades);
    newEvents = d.events.length;
    cur.events.push(...d.events);
    if (cur.events.length > MAX_ACTIVITY) cur.events.splice(0, cur.events.length - MAX_ACTIVITY);
    cur.lastSeq = d.last_seq;
  }
  live.last = d;
  renderDetail(d, { first, newBars, newEvents, newTrades });
}

function kpi(label, value, cls, sub) {
  const card = statCard(label, value, cls, sub);
  return card;
}

function renderDetail(d, change) {
  const cur = live.cur;
  const s = d.summary;
  const st = d.stats;
  $('dep-name').textContent = s.name;
  const bd = $('dep-badge');
  bd.replaceWith(Object.assign(badge(s.status), { id: 'dep-badge' }));
  $('dep-reason').textContent = s.reason || '';
  const uptime = s.started_ms ? fmtDuration((s.ended_ms || Date.now()) - s.started_ms) : '–';
  $('dep-meta').textContent = `${s.feed} · ${s.strategy_file || 'editor buffer'} · ${s.machine} · up ${uptime} · last bar ${s.last_label || '–'} (${ago(s.last_bar_ms)})`;

  const status = s.status;
  const isFinal = status === 'stopped' || status === 'finished' || status === 'error';
  $('dep-pause').hidden = status !== 'running';
  $('dep-resume').hidden = status !== 'paused';
  $('dep-flatten').hidden = isFinal;
  $('dep-stop').hidden = isFinal;
  $('dep-limits').hidden = isFinal;
  $('dep-delete').hidden = !isFinal;

  const banner = $('dep-banner');
  const notes = [];
  if (d.feed_health.consecutive_failures) notes.push(`Feed problems (${d.feed_health.consecutive_failures} in a row): ${d.feed_health.last_error}`);
  for (const w of d.warnings.slice(-3)) notes.push(w);
  banner.hidden = !notes.length;
  banner.textContent = notes.join('\n');

  const unreal = d.positions.reduce((a, p) => a + p.unrealized_pnl, 0);
  const sign = (x) => (x > 0 ? 'good' : x < 0 ? 'bad' : '');
  const kp = $('dep-kpis');
  kp.textContent = '';
  kp.append(
    kpi('Equity', fmtMoney(s.equity), '', 'from ' + fmtMoney(s.initial_cash)),
    kpi('Return', (s.return_pct >= 0 ? '+' : '') + fmtNum(s.return_pct) + '%', sign(s.return_pct), 'buy & hold ' + fmtNum(s.buy_hold_pct) + '%'),
    kpi('Drawdown', fmtNum(s.drawdown_pct) + '%', s.drawdown_pct > 0 ? 'bad' : '', 'max ' + fmtNum(s.max_drawdown_pct) + '%' + (d.limits.max_drawdown_pct ? ` · limit ${d.limits.max_drawdown_pct}%` : '')),
    kpi('Realised P&L', fmtNum(st.realized_pnl), sign(st.realized_pnl)),
    kpi('Unrealised P&L', fmtNum(unreal), sign(unreal), d.positions.length + ' open'),
    kpi('Cash', fmtMoney(d.cash)),
    kpi('Fills', String(st.fills), '', `${st.closed_trades} closed · win ${st.win_rate_pct === null ? '–' : fmtNum(st.win_rate_pct, 0) + '%'}`),
    kpi('Time invested', fmtNum(st.exposure_pct, 0) + '%'),
    kpi('Bars', String(s.bars), '', s.last_price ? 'last ' + fmtNum(s.last_price, 4) : ''),
  );

  const bp = $('dep-broker');
  const viaBroker = !!d.broker;
  bp.hidden = !viaBroker;
  $('dep-orders-wrap').hidden = !viaBroker;
  $('dep-cancel-orders').hidden = !viaBroker || isFinal;
  if (viaBroker) {
    const br = d.broker;
    bp.className = 'broker-panel' + (br.environment === 'live' ? ' live' : '');
    bp.textContent = '';
    const item = (label, value) => { const sp = el('span'); sp.append(document.createTextNode(label + ' '), el('b', { textContent: value })); return sp; };
    bp.append(el('b', { textContent: br.name }), envPill(br.environment));
    if (br.account) bp.append(item('cash', fmtMoney(br.account.cash)), item('equity', fmtMoney(br.account.equity)), item('buying power', fmtMoney(br.account.buying_power)));
    bp.append(item('working orders', String(br.working_orders)));
    if (br.kill_switch) bp.append(el('b', { className: 'bad', textContent: 'KILL SWITCH ON: buying is stopped' }));
    if (br.error) bp.append(el('b', { className: 'bad', textContent: br.error }));
    const body = document.querySelector('#dep-orders tbody');
    body.textContent = '';
    for (const o of d.orders) {
      const tr = el('tr');
      const kind = o.kind.type === 'market' ? 'market' : `${o.kind.type} ${o.kind.limit ?? o.kind.stop}`;
      for (const [t, c] of [[o.id], [o.side, o.side], [o.symbol], [kind], [fmtNum(o.qty, 4)], [fmtNum(o.filled_qty, 4)], [o.filled_qty ? fmtNum(o.avg_price, 4) : ''], [o.state.replace('_', ' '), 'st-' + o.state], [o.message || '']]) tr.append(el('td', { textContent: t, className: c || '' }));
      body.append(tr);
    }
    if (!d.orders.length) { const tr = el('tr'); tr.append(el('td', { colSpan: 9, className: 'muted', textContent: 'No orders yet.' })); body.append(tr); }
  }

  const pos = document.querySelector('#dep-positions tbody');
  pos.textContent = '';
  for (const p of d.positions) {
    const tr = el('tr');
    for (const [t, c] of [[p.symbol], [fmtNum(p.shares, 4)], [fmtNum(p.avg_cost, 4)], [fmtNum(p.last_price, 4)], [fmtNum(p.value)], [fmtNum(p.unrealized_pnl), p.unrealized_pnl > 0 ? 'buy' : p.unrealized_pnl < 0 ? 'sell' : '']]) tr.append(el('td', { textContent: t, className: c || '' }));
    pos.append(tr);
  }
  if (!d.positions.length) { const tr = el('tr'); tr.append(el('td', { colSpan: 6, className: 'muted', textContent: 'flat' })); pos.append(tr); }

  const procs = document.querySelector('#dep-procs tbody');
  procs.textContent = '';
  for (const p of d.processes) {
    const tr = el('tr');
    const stateCell = el('td');
    stateCell.append(el('i', { style: `display:inline-block;width:9px;height:9px;border-radius:2px;margin-right:6px;background:${colorFor(p.state)}` }), document.createTextNode(p.state));
    tr.append(el('td', { textContent: p.name }), stateCell, el('td', { textContent: p.wakes_every_bar ? 'every bar' : `every ${p.wake_every_bars} bar${p.wake_every_bars === 1 ? '' : 's'}` }));
    procs.append(tr);
  }

  if (change.first || change.newBars) {
    const data = {
      symbol: s.symbols[0], offset: cur.offset, labels: cur.labels, close: cur.close, equity: cur.equity, drawdown: cur.drawdown,
      buyHold: cur.firstClose ? cur.close.map((c) => (s.initial_cash / cur.firstClose) * c) : cur.close.map(() => s.initial_cash), trades: cur.trades,
    };
    showCharts('lv', { price: 'lv-price', equity: 'lv-equity', dd: 'lv-dd' }, data);
    drawStrips($('lv-strip'), cur.timelines, cur.labels);
  } else if (change.newTrades) {
    // a manual flatten can add trades without a new bar
    const price = state.charts.lvprice;
    if (price) showCharts('lv', { price: 'lv-price', equity: 'lv-equity', dd: 'lv-dd' }, {
      symbol: s.symbols[0], offset: cur.offset, labels: cur.labels, close: cur.close, equity: cur.equity, drawdown: cur.drawdown,
      buyHold: cur.firstClose ? cur.close.map((c) => (s.initial_cash / cur.firstClose) * c) : cur.close.map(() => s.initial_cash), trades: cur.trades,
    });
  }
  if (change.first || change.newEvents || change.filterChanged) renderActivity();
  if (change.first || change.newTrades) renderTrades();
}

const EVENT_GROUPS = {
  fill: 'fill', order: 'fill', cancel: 'control', state: 'state', risk: 'alert', error: 'alert', warning: 'alert', feed: 'alert',
  deployed: 'control', started: 'control', paused: 'control', resumed: 'control', stopped: 'control', finished: 'control', flatten: 'control', limits: 'control', restored: 'control',
};
const EVENT_ICONS = { order: '◆', state: '→', risk: '⚠', warning: '⚠', error: '✖', feed: '⚡', log: '…', fill: '●' };

function renderActivity() {
  const ul = $('activity');
  ul.textContent = '';
  const f = live.filter;
  let shown = 0;
  for (let i = live.cur.events.length - 1; i >= 0 && shown < 300; i--) {
    const e = live.cur.events[i];
    const group = EVENT_GROUPS[e.kind] || 'other';
    if (f !== 'all' && group !== f) continue;
    const side = e.kind === 'fill' && e.data ? e.data.side : '';
    const li = el('li', { className: `k-${e.kind} ${side}` });
    const icon = e.kind === 'fill' ? (side === 'buy' ? '▲' : '▼') : EVENT_ICONS[e.kind] || '●';
    li.append(el('span', { className: 'ts', textContent: new Date(e.ts_ms).toLocaleTimeString([], { hour12: false }) }), el('span', { className: 'ic', textContent: icon }), el('span', { className: 'msg', textContent: e.message }));
    ul.append(li);
    shown++;
  }
  if (!shown) ul.append(el('li', { className: 'empty', textContent: 'Nothing here yet.' }));
}

function renderTrades() {
  const body = document.querySelector('#dep-trades tbody');
  body.textContent = '';
  const trades = live.cur.trades;
  for (let i = trades.length - 1; i >= 0 && body.children.length < 300; i--) {
    const t = trades[i];
    const tr = el('tr');
    for (const [text, cls] of [[String(t.bar)], [t.label], [t.side, t.side], [fmtNum(t.shares, 4)], [fmtNum(t.price, 4)], [t.process], [t.state], [t.pnl === null ? '' : fmtNum(t.pnl), t.pnl > 0 ? 'buy' : t.pnl < 0 ? 'sell' : '']]) tr.append(el('td', { textContent: text, className: cls || '' }));
    body.append(tr);
  }
  if (!trades.length) { const tr = el('tr'); tr.append(el('td', { colSpan: 8, className: 'muted', textContent: 'No trades yet.' })); body.append(tr); }
}

// ---- live: controls -----------------------------------------------------------

function choose({ title, text, options }) {
  return new Promise((resolve) => {
    $('choice-title').textContent = title;
    $('choice-text').textContent = text;
    const box = $('choice-buttons');
    box.textContent = '';
    const done = (v) => { $('choice-modal').hidden = true; resolve(v); };
    for (const o of options) {
      const b = el('button', { className: 'btn ' + (o.cls || ''), textContent: o.label });
      b.addEventListener('click', () => done(o.id));
      box.append(b);
    }
    const cancel = el('button', { className: 'btn', textContent: 'Cancel' });
    cancel.addEventListener('click', () => done(null));
    box.append(cancel);
    $('choice-modal').hidden = false;
    $('choice-modal').dataset.cancel = '1';
    $('choice-modal').onclick = (ev) => { if (ev.target === $('choice-modal')) done(null); };
    live.cancelChoice = () => done(null);
  });
}

async function depAction(action, body = {}) {
  try {
    await api('POST', `/api/live/${encodeURIComponent(live.selected)}/${action}`, body);
  } catch (e) {
    say(e.message, 'err');
  }
  await liveRefresh().catch(() => {});
}

async function onStop() {
  const choice = await choose({
    title: 'Stop this deployment?',
    text: 'A stopped deployment is final: it cannot be resumed (deploy again instead). Open positions can be closed first.',
    options: [{ id: 'flatten', label: 'Stop and close positions', cls: 'danger' }, { id: 'keep', label: 'Stop, keep positions open' }],
  });
  if (choice) await depAction('stop', { flatten: choice === 'flatten' });
}

async function onFlatten() {
  const ok = await choose({ title: 'Close all positions?', text: 'Sells every open position at the latest price. The strategy keeps running and may open new positions.', options: [{ id: 'yes', label: 'Flatten now', cls: 'danger' }] });
  if (ok) await depAction('flatten');
}

async function onDelete() {
  const ok = await choose({ title: 'Delete this deployment?', text: 'Its history, trades and activity log are removed from the server. This cannot be undone.', options: [{ id: 'yes', label: 'Delete', cls: 'danger' }] });
  if (!ok) return;
  try {
    await api('DELETE', '/api/live/' + encodeURIComponent(live.selected));
    live.selected = null;
    live.cur = null;
    showDetail(false);
    await refreshList();
  } catch (e) {
    say(e.message, 'err');
  }
}

function openLimits() {
  const l = live.last && live.last.limits;
  $('lm-dd').value = l && l.max_drawdown_pct ? l.max_drawdown_pct : '';
  $('lm-pos').value = l && l.max_position_value ? l.max_position_value : '';
  $('lm-error').hidden = true;
  $('limits-modal').hidden = false;
}

async function saveLimits(ev) {
  ev.preventDefault();
  const num = (id) => ($(id).value.trim() === '' ? null : Number($(id).value));
  try {
    await api('PUT', `/api/live/${encodeURIComponent(live.selected)}/limits`, { max_drawdown_pct: num('lm-dd'), max_position_value: num('lm-pos') });
    $('limits-modal').hidden = true;
    await liveRefresh();
  } catch (e) {
    $('lm-error').hidden = false;
    $('lm-error').textContent = e.message;
  }
}

async function openDeployedSource() {
  if (!confirmDiscard()) return;
  try {
    const r = await api('GET', `/api/live/${encodeURIComponent(live.selected)}/source`);
    state.loading = true;
    editor.setValue(r.source);
    state.loading = false;
    state.file = null;
    state.machine = r.machine || null;
    $('machine-select').dataset.key = '';
    $('file-name').textContent = `snapshot of “${live.last ? live.last.summary.name : 'deployment'}” (unsaved)`;
    state.testsFile = null;
    $('tests-text').value = '';
    setDirty(true);
    setView('editor');
    analyze();
  } catch (e) {
    say(e.message, 'err');
  }
}

// ---- live: deploy dialog --------------------------------------------------------

function selectedConnection() {
  const name = $('dp-exec').value;
  return live.brokers.list.find((c) => c.name === name) || null;
}

function fillExecOptions() {
  const sel = $('dp-exec');
  const prev = sel.value;
  sel.textContent = '';
  sel.append(el('option', { value: '', textContent: 'Simulated account (paper)' }));
  for (const c of live.brokers.list) {
    const o = el('option', { value: c.name, textContent: `${c.name} (${c.environment || 'invalid'})${c.ready ? '' : ' - not ready'}` });
    o.disabled = !c.ready;
    sel.append(o);
  }
  sel.value = [...sel.options].some((o) => o.value === prev && !o.disabled) ? prev : '';
}

function updateExecFields() {
  const c = selectedConnection();
  $('dp-exec-broker').hidden = !c;
  $('dp-cash-wrap').hidden = !!c;
  $('dp-fee-wrap').hidden = !!c;
  const conf = $('dp-confirm-wrap');
  conf.hidden = !(c && c.environment === 'live');
  if (c) {
    $('dp-confirm-label').textContent = `This trades REAL MONEY. Type “${c.name}” to confirm:`;
    const lim = c.limits || {};
    $('dp-exec-note').textContent = `Connection limits: ${lim.max_order_value ? 'order ≤ ' + fmtMoney(lim.max_order_value) : 'no order limit'}, ${lim.max_position_value ? 'position ≤ ' + fmtMoney(lim.max_position_value) : 'no position limit'}${lim.max_orders ? ', ≤ ' + lim.max_orders + ' orders' : ''}. Your values can only tighten them. The deployment starts from the account's current equity; it manages only what it buys itself.`;
  }
  $('dp-disclaimer').textContent = !c
    ? 'Paper trading: orders are simulated against the feed and never reach a broker. The strategy runs in the preview simulator, which may differ from the C++ runtime.'
    : (c.environment === 'live'
      ? 'Orders will be sent to a real brokerage account. Limits are enforced by the server, but the strategy itself is software you wrote: test it on a paper connection first, start with tiny limits, and watch it.'
      : 'Orders will be sent to the brokerage\'s paper/sandbox environment. Prices, fills and behaviour can still differ from live trading.');
}

function updateDeployFields() {
  fillExecOptions();
  updateExecFields();
  const kind = $('dp-feed').value;
  const synthetic = kind !== 'twelve_data' && kind !== 'broker';
  $('dp-symbol-wrap').hidden = !synthetic;
  $('dp-symbols-wrap').hidden = synthetic;
  $('dp-tick-wrap').hidden = !synthetic;
  $('dp-interval-wrap').hidden = synthetic;
  $('dp-gen').hidden = !synthetic;
  $('dp-bars-wrap').hidden = kind !== 'replay';
  $('dp-csv-wrap').hidden = kind !== 'replay';
  $('dp-barsize-wrap').hidden = !synthetic;
  const note = $('dp-feed-note');
  $('dp-interval').min = kind === 'broker' ? '0.25' : '5';
  $('dp-interval').step = kind === 'broker' ? 'any' : '1';
  if (kind === 'broker') {
    $('dp-symbols-wrap').hidden = false;
    $('dp-interval-wrap').hidden = false;
    note.textContent = 'Samples the last price from the brokerage connection chosen under Execution (or the first ready one) at each interval; every sample is one bar. Orders always go to the connection under Execution.';
  } else if (kind === 'simulated') note.textContent = 'An endless synthetic price stream, one bar per interval. Deterministic for a seed, so it can be resumed exactly after a restart.';
  else if (kind === 'replay') note.textContent = 'Plays a price history back one bar per interval and finishes when it ends. A quick way to watch a strategy run through a past market.';
  else note.textContent = live.meta && live.meta.twelvedata
    ? 'Polls the latest price once per interval using the server\'s Twelve Data key. Bars are the polled prices.'
    : 'Needs TWELVEDATA_API_KEY in the server\'s environment, which is not set: the deployment will be refused.';
}

async function openDeploy() {
  if (!editor) return;
  const a = state.analysis;
  if (!a || !a.ok) { say('Fix the syntax error before deploying.', 'err'); return; }
  const errors = (a.diagnostics || []).filter((d) => d.severity === 'error');
  if (errors.length) { say(`Fix ${errors.length} error(s) before deploying: ${errors[0].message}`, 'err'); return; }
  try { await refreshList(); await refreshBrokers(); } catch { /* the dialog still works */ }
  const base = state.file ? baseName(state.file) : 'strategy';
  $('dp-name').value = base + (state.machine ? ` · ${state.machine}` : '');
  $('dp-source').textContent = `${state.file || 'unsaved buffer'}${state.machine ? ' · machine ' + state.machine : ''}${state.dirty ? ' (includes unsaved changes)' : ''}`;
  renderParams(a.params || [], 'dp-params');
  $('dp-error').hidden = true;
  updateDeployFields();
  $('deploy-modal').hidden = false;
  $('dp-name').focus();
}

function deployFeed() {
  const kind = $('dp-feed').value;
  const generator = {
    kind: $('dp-gen-kind').value,
    seed: Number($('dp-seed').value),
    start: Number($('dp-start-price').value),
    vol: Number($('dp-vol').value) / 100,
    period: Number($('dp-period').value),
  };
  const tick_ms = Math.round(Number($('dp-tick').value) * 1000);
  const symbol = $('dp-symbol').value.trim() || 'TEST';
  if (kind === 'simulated') return { kind, symbol, generator, tick_ms };
  if (kind === 'replay') {
    const csv = $('dp-csv').value.trim();
    const data = csv ? { source: 'csv', csv } : { source: 'generate', generator: { ...generator, bars: Number($('dp-bars').value) } };
    return { kind, symbol, data, tick_ms };
  }
  const symbols = $('dp-symbols').value.split(',').map((x) => x.trim()).filter(Boolean);
  if (kind === 'broker') {
    const c = selectedConnection() || live.brokers.list.find((x) => x.ready);
    return { kind, broker: c ? c.name : '', symbols, tick_ms: Math.round(Number($('dp-interval').value) * 1000) };
  }
  return { kind, symbols, interval_secs: Number($('dp-interval').value) };
}

async function submitDeploy(ev) {
  ev.preventDefault();
  const num = (id) => ($(id).value.trim() === '' ? null : Number($(id).value));
  const feed = deployFeed();
  const body = {
    name: $('dp-name').value,
    source: editor.getValue(),
    strategy_file: state.file,
    machine: state.machine,
    params: readParams('dp-params'),
    feed,
    initial_cash: Number($('dp-cash').value),
    fee_bps: Number($('dp-fee').value),
    max_position_value: num('dp-maxpos'),
    max_drawdown_pct: num('dp-maxdd'),
    start: $('dp-start').value === '1',
  };
  if (feed.kind === 'simulated' || feed.kind === 'replay') body.bar_seconds = Number($('dp-barsecs').value);
  const conn = selectedConnection();
  if (conn) {
    body.execution = { kind: 'broker', broker: conn.name, limits: { max_order_value: num('dp-lim-order'), max_position_value: num('dp-lim-pos'), max_orders: num('dp-lim-n') } };
    if (conn.environment === 'live') body.execution.confirm = $('dp-confirm').value.trim();
  }
  $('dp-submit').disabled = true;
  try {
    const s = await api('POST', '/api/live', body);
    $('deploy-modal').hidden = true;
    live.selected = s.id;
    live.cur = null;
    setView('live');
    await selectDeployment(s.id);
    say(`Deployed “${s.name}”`, 'ok');
  } catch (e) {
    $('dp-error').hidden = false;
    $('dp-error').textContent = e.message;
  } finally {
    $('dp-submit').disabled = false;
  }
}

// ---- wiring ------------------------------------------------------------------

function wire() {
  document.querySelectorAll('.tab').forEach((t) => t.addEventListener('click', () => switchTab(t.dataset.tab)));
  $('btn-save').addEventListener('click', () => saveFile());
  $('btn-new').addEventListener('click', () => createFile(NEW_TEMPLATE));
  $('btn-compile').addEventListener('click', compile);
  $('btn-run').addEventListener('click', runBacktest);
  $('btn-run2').addEventListener('click', runBacktest);
  $('btn-tests').addEventListener('click', () => runTests(true));
  $('btn-run-tests').addEventListener('click', () => runTests(true));
  $('btn-golden').addEventListener('click', addGoldenTest);
  $('btn-test-template').addEventListener('click', () => {
    $('tests-text').value = fmtJson(TEST_TEMPLATE);
    state.testsDirty = true;
  });
  $('tests-text').addEventListener('input', () => { state.testsDirty = true; });
  $('machine-select').addEventListener('change', (ev) => {
    state.machine = ev.target.value;
    state.lastMermaid = '';
    analyze();
  });
  $('data-kind').addEventListener('change', (ev) => {
    $('gen-fields').hidden = ev.target.value !== 'generate';
    $('csv-fields').hidden = ev.target.value !== 'csv';
  });
  $('csv-file').addEventListener('change', async (ev) => {
    const file = ev.target.files[0];
    if (file) $('csv-text').value = await file.text();
  });
  $('view-editor').addEventListener('click', () => setView('editor'));
  $('view-live').addEventListener('click', () => setView('live'));
  $('btn-deploy').addEventListener('click', openDeploy);
  $('view-cluster').addEventListener('click', () => setView('cluster'));
  $('btn-cluster-deploy').addEventListener('click', openClusterDeploy);
  $('cluster-form').addEventListener('submit', submitClusterDeploy);
  $('cl-cancel').addEventListener('click', () => { $('cluster-modal').hidden = true; if (live.view === 'cluster') refreshCluster().catch(() => {}); });
  $('live-new').addEventListener('click', () => { setView('editor'); openDeploy(); });
  $('dp-feed').addEventListener('change', updateDeployFields);
  $('dp-exec').addEventListener('change', () => { updateExecFields(); if ($('dp-feed').value === 'broker') updateDeployFields(); });
  $('kill-toggle').addEventListener('click', toggleKill);
  $('emergency').addEventListener('click', emergencyStop);
  $('bt-run').addEventListener('click', runBrokerTest);
  $('bt-close').addEventListener('click', () => { $('bt-modal').hidden = true; });
  $('dep-cancel-orders').addEventListener('click', () => depAction('cancel-orders'));
  $('deploy-form').addEventListener('submit', submitDeploy);
  $('dp-cancel').addEventListener('click', () => { $('deploy-modal').hidden = true; });
  $('limits-form').addEventListener('submit', saveLimits);
  $('lm-cancel').addEventListener('click', () => { $('limits-modal').hidden = true; });
  $('dep-pause').addEventListener('click', () => depAction('pause'));
  $('dep-resume').addEventListener('click', () => depAction('resume'));
  $('dep-flatten').addEventListener('click', onFlatten);
  $('dep-stop').addEventListener('click', onStop);
  $('dep-delete').addEventListener('click', onDelete);
  $('dep-limits').addEventListener('click', openLimits);
  $('dep-source').addEventListener('click', openDeployedSource);
  document.querySelectorAll('#act-filter .chip').forEach((c) => c.addEventListener('click', () => {
    live.filter = c.dataset.f;
    document.querySelectorAll('#act-filter .chip').forEach((x) => x.classList.toggle('active', x === c));
    if (live.cur) renderActivity();
  }));
  $('ai-generate').addEventListener('click', () => aiRun('generate'));
  $('ai-fix').addEventListener('click', () => aiRun('fix'));
  $('ai-explain').addEventListener('click', () => aiRun('explain'));
  $('ai-apply').addEventListener('click', () => {
    if (state.ai.source) {
      editor.executeEdits('ai', [{ range: editor.getModel().getFullModelRange(), text: state.ai.source }]);
      editor.pushUndoStop();
      say('Editor updated. Use undo to go back.', 'ok');
    }
  });
  $('ai-newfile').addEventListener('click', () => { if (state.ai.source) createFile(state.ai.source); });
  window.addEventListener('beforeunload', (ev) => {
    if (state.dirty || state.testsDirty) { ev.preventDefault(); ev.returnValue = ''; }
  });
  document.addEventListener('keydown', (ev) => {
    if ((ev.ctrlKey || ev.metaKey) && ev.key === 's') { ev.preventDefault(); saveFile(); }
    if (ev.key === 'Escape') {
      if (!$('modal').hidden) $('modal-cancel').click();
      if (!$('deploy-modal').hidden) $('dp-cancel').click();
      if (!$('limits-modal').hidden) $('lm-cancel').click();
      if (!$('bt-modal').hidden) $('bt-close').click();
      if (!$('cluster-modal').hidden) $('cl-cancel').click();
      if (!$('choice-modal').hidden && live.cancelChoice) live.cancelChoice();
    }
  });
}

async function main() {
  window.mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    theme: 'base',
    themeVariables: {
      darkMode: true,
      background: '#0f0d0c',
      fontFamily: 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
      fontSize: '13px',
      primaryColor: '#241b15',
      primaryTextColor: '#ece6e0',
      primaryBorderColor: '#a9531a',
      lineColor: '#bba99b',
      secondaryColor: '#1d1916',
      tertiaryColor: '#171311',
      clusterBkg: '#171311',
      clusterBorder: '#3a312b',
      edgeLabelBackground: '#171311',
      titleColor: '#9c9087',
    },
    flowchart: { htmlLabels: true, curve: 'basis', useMaxWidth: true, nodeSpacing: 36, rankSpacing: 42 },
  });
  wire();
  await bootEditor();
  await loadFiles();
  loadAiStatus();
  refreshList().catch(() => {});
  initCluster().then(() => { if (cluster.configured) refreshCluster().catch(() => {}); });
  setInterval(() => { if (live.view !== 'live' && !document.hidden) refreshList().catch(() => {}); }, 5000);
  const first = state.files.find((f) => f.name === 'threshold.sm') || state.files.find((f) => f.kind === 'strategy');
  if (first) await openFile(first.name);
  else say('Create a strategy with “New”.');
}

main().catch((e) => say('Startup failed: ' + e.message, 'err'));
