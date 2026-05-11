// Why: vanilla JS entry point for the trusty-analyzer dashboard. Keeps the
// embedded UI small (no framework runtime) so the Rust binary stays compact.
// What: bootstraps API client, tabs, auto-refresh, and the four
// visualizations (complexity treemap + hotspots, smells bar chart, refactor
// list, cluster grid).
// Test: open the daemon at http://127.0.0.1:7879/ui/ and verify each tab
// renders without console errors when trusty-search is reachable.

(() => {
  'use strict';

  // ---------------- config + small helpers ----------------
  const API_BASE = `http://${location.hostname || '127.0.0.1'}:${window.__DAEMON_PORT__ || 7879}`;
  // If served from same origin (the daemon at :7879), prefer relative URLs.
  const SAME_ORIGIN = location.port === String(window.__DAEMON_PORT__ || 7879);
  const apiUrl = (path) => (SAME_ORIGIN ? path : `${API_BASE}${path}`);

  const REFRESH_MS = 30000;
  const GRADES = ['A', 'B', 'C', 'D', 'F'];

  /** Why: centralize fetch + error handling for the daemon API.
   *  What: returns parsed JSON or throws with a useful message.
   *  Test: stop the daemon and call apiGet('/health') — should throw. */
  async function apiGet(path) {
    const r = await fetch(apiUrl(path), { headers: { Accept: 'application/json' } });
    if (!r.ok) throw new Error(`${r.status} ${r.statusText}: ${path}`);
    return r.json();
  }

  function el(id) { return document.getElementById(id); }

  function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

  function gradeFromComplexity(cyclo) {
    if (cyclo <= 5) return 'A';
    if (cyclo <= 10) return 'B';
    if (cyclo <= 20) return 'C';
    if (cyclo <= 40) return 'D';
    return 'F';
  }

  function gradeColor(grade) {
    return ({
      A: '#a6e3a1',
      B: '#94e2d5',
      C: '#f9e2af',
      D: '#fab387',
      F: '#f38ba8',
    })[grade] || '#6c7290';
  }

  function severityRank(sev) {
    return { low: 0, medium: 1, high: 2, critical: 3 }[String(sev || '').toLowerCase()] ?? 0;
  }

  function basename(p) {
    if (!p) return '';
    const s = String(p);
    const i = s.lastIndexOf('/');
    return i >= 0 ? s.slice(i + 1) : s;
  }

  function fmt(n, digits = 1) {
    if (n == null || Number.isNaN(n)) return '-';
    if (typeof n !== 'number') return String(n);
    return Number.isInteger(n) ? String(n) : n.toFixed(digits);
  }

  // ---------------- app state ----------------
  const state = {
    indexes: [],
    currentIndex: null,
    hotspots: [],
    smells: [],
    quality: null,
    refactors: [],
    clusters: [],
    refreshTimer: null,
  };

  // ---------------- DOM bootstrap ----------------
  function initTabs() {
    document.querySelectorAll('.tab').forEach((btn) => {
      btn.addEventListener('click', () => {
        const target = btn.dataset.tab;
        document.querySelectorAll('.tab').forEach((b) => b.classList.toggle('active', b === btn));
        document.querySelectorAll('.tab-panel').forEach((p) => {
          p.classList.toggle('active', p.id === `tab-${target}`);
        });
        // Redraw responsive charts when tab becomes visible.
        if (target === 'complexity') renderTreemap();
        if (target === 'smells') renderSmellsChart();
      });
    });
  }

  function initIndexPicker() {
    el('index-select').addEventListener('change', (e) => {
      state.currentIndex = e.target.value || null;
      loadIndexData();
    });
  }

  function initRefactorFilter() {
    el('refactor-severity').addEventListener('change', () => {
      renderRefactors();
    });
  }

  // ---------------- error banner ----------------
  function setError(message) {
    const banner = el('error-banner');
    if (message) {
      banner.classList.remove('hidden');
      banner.querySelector('strong').textContent = message;
    } else {
      banner.classList.add('hidden');
    }
  }

  function setHealthPill(state) {
    const pill = el('health-pill');
    pill.classList.remove('pill-ok', 'pill-warn', 'pill-bad', 'pill-unknown');
    if (state === 'ok') { pill.classList.add('pill-ok'); pill.textContent = 'healthy'; }
    else if (state === 'warn') { pill.classList.add('pill-warn'); pill.textContent = 'degraded'; }
    else if (state === 'bad') { pill.classList.add('pill-bad'); pill.textContent = 'offline'; }
    else { pill.classList.add('pill-unknown'); pill.textContent = '...'; }
  }

  // ---------------- health + indexes ----------------
  async function checkHealth() {
    try {
      const h = await apiGet('/health');
      if (h.status === 'ok' && h.search_reachable !== false) {
        setHealthPill('ok');
        setError(null);
        return true;
      }
      if (h.search_reachable === false) {
        setHealthPill('warn');
        setError('trusty-search not reachable.');
        return false;
      }
      setHealthPill('warn');
      return true;
    } catch (e) {
      setHealthPill('bad');
      setError('trusty-analyzer daemon not reachable.');
      return false;
    }
  }

  async function loadIndexes() {
    try {
      const indexes = await apiGet('/indexes');
      state.indexes = Array.isArray(indexes) ? indexes : [];
      const select = el('index-select');
      const previous = state.currentIndex;
      clear(select);
      if (state.indexes.length === 0) {
        const opt = document.createElement('option');
        opt.textContent = '(no indexes available)';
        opt.value = '';
        select.appendChild(opt);
        state.currentIndex = null;
        return;
      }
      for (const idx of state.indexes) {
        const opt = document.createElement('option');
        opt.value = idx.id;
        opt.textContent = idx.name ? `${idx.name} (${idx.id})` : idx.id;
        select.appendChild(opt);
      }
      if (previous && state.indexes.some((i) => i.id === previous)) {
        state.currentIndex = previous;
        select.value = previous;
      } else {
        state.currentIndex = state.indexes[0].id;
        select.value = state.currentIndex;
      }
    } catch (e) {
      console.warn('loadIndexes failed:', e);
    }
  }

  // ---------------- per-index data loader ----------------
  async function loadIndexData() {
    if (!state.currentIndex) return;
    const id = encodeURIComponent(state.currentIndex);

    const tasks = [
      apiGet(`/indexes/${id}/complexity_hotspots?top_k=40`).then((r) => { state.hotspots = r || []; }).catch(() => { state.hotspots = []; }),
      apiGet(`/indexes/${id}/smells`).then((r) => { state.smells = r || []; }).catch(() => { state.smells = []; }),
      apiGet(`/indexes/${id}/quality`).then((r) => { state.quality = r || null; }).catch(() => { state.quality = null; }),
      apiGet(`/indexes/${id}/refactor-suggestions?min_severity=low&top_k=40`).then((r) => { state.refactors = r || []; }).catch(() => { state.refactors = []; }),
      apiGet(`/indexes/${id}/clusters?k=8&method=bow`).then((r) => { state.clusters = r || []; }).catch(() => { state.clusters = []; }),
    ];
    await Promise.all(tasks);

    renderQuality();
    renderSidebarHotspots();
    renderTreemap();
    renderHotspotTable();
    renderSmellsChart();
    renderSmellsList();
    renderRefactors();
    renderClusters();
  }

  // ---------------- quality card ----------------
  function renderQuality() {
    const letter = el('grade-letter');
    letter.classList.remove(...GRADES.map((g) => `grade-${g}`), 'grade-unknown');
    if (!state.quality) {
      letter.textContent = '-';
      letter.classList.add('grade-unknown');
      el('quality-avg').textContent = '-';
      el('quality-pct-a').textContent = '-';
      el('quality-smells').textContent = '-';
      return;
    }
    const grade = String(state.quality.grade || '').toUpperCase() || gradeFromComplexity(state.quality.avg_cyclomatic || 0);
    letter.textContent = grade || '-';
    letter.classList.add(`grade-${grade}`);
    el('quality-avg').textContent = fmt(state.quality.avg_cyclomatic, 2);
    const pct = state.quality.pct_grade_a;
    el('quality-pct-a').textContent = pct == null ? '-' : `${(pct <= 1 ? pct * 100 : pct).toFixed(1)}%`;
    el('quality-smells').textContent = fmt(state.quality.smell_count);
  }

  // ---------------- sidebar hotspot list ----------------
  function renderSidebarHotspots() {
    const list = el('sidebar-hotspots');
    clear(list);
    const top = state.hotspots.slice(0, 10);
    if (!top.length) {
      const li = document.createElement('li');
      li.className = 'empty-state';
      li.textContent = 'no data';
      list.appendChild(li);
      return;
    }
    for (const h of top) {
      const li = document.createElement('li');
      const file = document.createElement('span');
      file.className = 'file';
      file.textContent = basename(h.file || h.file_path || '?');
      file.title = h.file || h.file_path || '';
      const score = document.createElement('span');
      score.className = 'score';
      score.textContent = fmt(h.cyclomatic ?? h.complexity ?? h.score, 0);
      li.appendChild(file);
      li.appendChild(score);
      list.appendChild(li);
    }
  }

  // ---------------- complexity treemap (D3) ----------------
  function renderTreemap() {
    const container = el('treemap');
    if (!container || !window.d3) return;
    clear(container);

    if (!state.hotspots.length) {
      container.innerHTML = '<div class="empty-state">no complexity data for this index</div>';
      return;
    }

    // Group hotspots by file and sum complexity (treemap input).
    const byFile = new Map();
    for (const h of state.hotspots) {
      const f = h.file || h.file_path || '(unknown)';
      const c = Number(h.cyclomatic ?? h.complexity ?? h.score ?? 1) || 1;
      const entry = byFile.get(f) || { file: f, total: 0, max: 0, children: [] };
      entry.total += c;
      entry.max = Math.max(entry.max, c);
      entry.children.push(h);
      byFile.set(f, entry);
    }

    const data = {
      name: 'root',
      children: Array.from(byFile.values()).map((f) => ({
        name: basename(f.file),
        full: f.file,
        value: f.total,
        grade: gradeFromComplexity(f.max),
        max: f.max,
        chunks: f.children,
      })),
    };

    const rect = container.getBoundingClientRect();
    const width = Math.max(320, rect.width || 800);
    const height = Math.max(280, rect.height || 460);

    const svg = d3.select(container).append('svg')
      .attr('viewBox', `0 0 ${width} ${height}`)
      .attr('preserveAspectRatio', 'xMidYMid meet');

    const root = d3.hierarchy(data).sum((d) => d.value || 0).sort((a, b) => b.value - a.value);
    d3.treemap().size([width, height]).padding(2).round(true)(root);

    const nodes = svg.selectAll('g.treemap-node')
      .data(root.leaves())
      .enter()
      .append('g')
      .attr('class', 'treemap-node')
      .attr('transform', (d) => `translate(${d.x0},${d.y0})`);

    nodes.append('rect')
      .attr('width', (d) => Math.max(0, d.x1 - d.x0))
      .attr('height', (d) => Math.max(0, d.y1 - d.y0))
      .attr('fill', (d) => gradeColor(d.data.grade))
      .attr('rx', 3)
      .on('click', (_event, d) => showTreemapDetail(d.data));

    nodes.append('title')
      .text((d) => `${d.data.full}\nGrade ${d.data.grade} - peak cyclomatic ${d.data.max}`);

    nodes.append('text')
      .attr('x', 6)
      .attr('y', 14)
      .text((d) => {
        const w = d.x1 - d.x0;
        const label = d.data.name;
        if (w < 50) return '';
        const maxChars = Math.max(4, Math.floor(w / 7));
        return label.length > maxChars ? label.slice(0, maxChars - 1) + '…' : label;
      });
  }

  function showTreemapDetail(d) {
    const detail = el('treemap-detail');
    if (!d) {
      detail.textContent = 'Click a tile to see chunk details.';
      return;
    }
    clear(detail);
    const header = document.createElement('div');
    const path = document.createElement('span');
    path.className = 'file-path mono';
    path.textContent = d.full;
    header.appendChild(path);
    detail.appendChild(header);

    const meta = document.createElement('div');
    meta.className = 'small';
    meta.style.marginTop = '6px';
    meta.innerHTML = `Grade <strong class="grade-${d.grade}">${d.grade}</strong> - peak cyclomatic <strong>${d.max}</strong> - ${d.chunks.length} chunk(s)`;
    detail.appendChild(meta);

    const topChunks = d.chunks.slice(0, 5);
    const list = document.createElement('ul');
    list.style.margin = '8px 0 0 16px';
    list.style.padding = '0';
    for (const c of topChunks) {
      const li = document.createElement('li');
      const fn = c.function_name || c.function || '(top-level)';
      const lines = c.start_line != null ? ` lines ${c.start_line}-${c.end_line ?? '?'}` : '';
      li.innerHTML = `<code>${fn}</code>${lines} - cyclomatic <strong>${c.cyclomatic ?? c.complexity ?? '?'}</strong>`;
      list.appendChild(li);
    }
    detail.appendChild(list);
  }

  // ---------------- hotspot table ----------------
  function renderHotspotTable() {
    const wrap = el('hotspot-table');
    clear(wrap);
    if (!state.hotspots.length) {
      wrap.innerHTML = '<div class="empty-state">no hotspots</div>';
      return;
    }
    const maxC = Math.max(...state.hotspots.map((h) => Number(h.cyclomatic ?? h.complexity ?? h.score ?? 0))) || 1;
    for (const h of state.hotspots.slice(0, 20)) {
      const cyclo = Number(h.cyclomatic ?? h.complexity ?? h.score ?? 0) || 0;
      const grade = gradeFromComplexity(cyclo);
      const row = document.createElement('div');
      row.className = 'hotspot-row';
      row.innerHTML = `
        <span class="file" title="${h.file || ''}">${h.file || '?'}</span>
        <span class="fn">${h.function_name || h.function || '(top-level)'}</span>
        <span class="bar"><span class="bar-fill" style="width:${(cyclo / maxC * 100).toFixed(1)}%"></span></span>
        <span class="grade-badge grade-${grade}">${cyclo}</span>
      `;
      wrap.appendChild(row);
    }
  }

  // ---------------- smells ----------------
  function renderSmellsChart() {
    const container = el('smells-chart');
    if (!container || !window.d3) return;
    clear(container);
    if (!state.smells.length) {
      container.innerHTML = '<div class="empty-state">no smells detected</div>';
      return;
    }
    const counts = new Map();
    for (const s of state.smells) {
      const key = s.category || s.name || s.smell || 'other';
      counts.set(key, (counts.get(key) || 0) + 1);
    }
    const data = Array.from(counts, ([category, count]) => ({ category, count }))
      .sort((a, b) => b.count - a.count);

    const rect = container.getBoundingClientRect();
    const width = Math.max(320, rect.width || 700);
    const height = 280;
    const margin = { top: 16, right: 16, bottom: 60, left: 40 };

    const svg = d3.select(container).append('svg')
      .attr('viewBox', `0 0 ${width} ${height}`)
      .attr('preserveAspectRatio', 'xMidYMid meet');

    const x = d3.scaleBand()
      .domain(data.map((d) => d.category))
      .range([margin.left, width - margin.right])
      .padding(0.2);

    const y = d3.scaleLinear()
      .domain([0, d3.max(data, (d) => d.count) || 1])
      .nice()
      .range([height - margin.bottom, margin.top]);

    svg.append('g').attr('class', 'axis')
      .attr('transform', `translate(0,${height - margin.bottom})`)
      .call(d3.axisBottom(x))
      .selectAll('text')
        .attr('transform', 'rotate(-30)')
        .style('text-anchor', 'end');

    svg.append('g').attr('class', 'axis')
      .attr('transform', `translate(${margin.left},0)`)
      .call(d3.axisLeft(y).ticks(5));

    svg.selectAll('.bar')
      .data(data)
      .enter()
      .append('rect')
      .attr('class', 'bar')
      .attr('x', (d) => x(d.category))
      .attr('y', (d) => y(d.count))
      .attr('width', x.bandwidth())
      .attr('height', (d) => height - margin.bottom - y(d.count))
      .append('title')
      .text((d) => `${d.category}: ${d.count}`);
  }

  function renderSmellsList() {
    const wrap = el('smells-list');
    const counter = el('smells-count');
    clear(wrap);
    counter.textContent = state.smells.length ? `${state.smells.length} smell(s)` : '';
    if (!state.smells.length) {
      wrap.innerHTML = '<div class="empty-state">no smells</div>';
      return;
    }
    for (const s of state.smells.slice(0, 100)) {
      const row = document.createElement('div');
      row.className = 'smell-row';
      const head = document.createElement('div');
      head.className = 'smell-head';
      const name = document.createElement('span');
      name.className = 'smell-name';
      name.textContent = s.name || s.category || 'smell';
      const file = document.createElement('span');
      file.className = 'mono muted small';
      file.textContent = `${s.file || '?'}${s.function_name ? ' :: ' + s.function_name : ''}`;
      head.appendChild(name);
      head.appendChild(file);
      row.appendChild(head);
      if (s.description || s.message) {
        const desc = document.createElement('div');
        desc.className = 'smell-desc';
        desc.textContent = s.description || s.message;
        row.appendChild(desc);
      }
      wrap.appendChild(row);
    }
  }

  // ---------------- refactors ----------------
  function renderRefactors() {
    const wrap = el('refactor-list');
    clear(wrap);
    const minRank = severityRank(el('refactor-severity').value);
    const filtered = state.refactors.filter((r) => severityRank(r.severity) >= minRank);
    if (!filtered.length) {
      wrap.innerHTML = '<div class="empty-state">no refactor suggestions at this severity</div>';
      return;
    }
    for (const r of filtered) {
      const sev = String(r.severity || 'low').toLowerCase();
      const card = document.createElement('div');
      card.className = `refactor-card sev-${sev}`;
      const head = document.createElement('div');
      head.className = 'refactor-head';
      head.innerHTML = `
        <span class="sev-badge sev-${sev}">${sev}</span>
        <span class="refactor-type">${r.refactor_type || r.type || 'refactor'}</span>
        <span class="file-path">${r.file || '?'}${r.start_line ? `:${r.start_line}` : ''}</span>
        <span class="fn-name">${r.function_name || r.function || ''}</span>
      `;
      card.appendChild(head);
      if (r.rationale || r.reason) {
        const p = document.createElement('div');
        p.className = 'refactor-rationale';
        p.textContent = r.rationale || r.reason;
        card.appendChild(p);
      }
      if (r.suggested_action || r.action) {
        const a = document.createElement('div');
        a.className = 'refactor-action';
        a.innerHTML = `<strong>Suggested:</strong> ${r.suggested_action || r.action}`;
        card.appendChild(a);
      }
      wrap.appendChild(card);
    }
  }

  // ---------------- clusters ----------------
  function renderClusters() {
    const wrap = el('clusters-grid');
    clear(wrap);
    if (!state.clusters.length) {
      wrap.innerHTML = '<div class="empty-state">no clusters available</div>';
      return;
    }
    for (const c of state.clusters) {
      const card = document.createElement('div');
      card.className = 'cluster-card';
      const label = document.createElement('div');
      label.className = 'cluster-label';
      label.textContent = c.label || c.name || `cluster ${c.id ?? ''}`;
      const count = document.createElement('div');
      count.className = 'cluster-count';
      const n = (c.chunk_ids && c.chunk_ids.length) || c.size || c.count || 0;
      count.textContent = `${n} chunk${n === 1 ? '' : 's'}`;
      const terms = document.createElement('div');
      terms.className = 'cluster-terms';
      const termList = c.centroid_terms || c.terms || c.keywords || [];
      for (const t of termList.slice(0, 12)) {
        const span = document.createElement('span');
        span.className = 'cluster-term';
        span.textContent = typeof t === 'string' ? t : (t.term || t.word || '');
        terms.appendChild(span);
      }
      card.appendChild(label);
      card.appendChild(count);
      card.appendChild(terms);
      wrap.appendChild(card);
    }
  }

  // ---------------- auto-refresh + bootstrap ----------------
  async function refreshAll() {
    const healthy = await checkHealth();
    if (!healthy) return;
    await loadIndexes();
    await loadIndexData();
  }

  function scheduleRefresh() {
    if (state.refreshTimer) clearInterval(state.refreshTimer);
    state.refreshTimer = setInterval(refreshAll, REFRESH_MS);
  }

  function bootstrap() {
    initTabs();
    initIndexPicker();
    initRefactorFilter();
    // Defer the first draw slightly so D3 (loaded via <script>) finishes parsing.
    const start = () => { refreshAll(); scheduleRefresh(); };
    if (window.d3) start();
    else window.addEventListener('load', start, { once: true });
    window.addEventListener('resize', () => {
      // Re-render charts that depend on container width.
      renderTreemap();
      renderSmellsChart();
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', bootstrap, { once: true });
  } else {
    bootstrap();
  }
})();
