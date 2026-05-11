<script>
  /*
   * Why: Landing page surfaces the four most important static-analysis
   * metrics for the currently selected index plus a quick look at the
   * top-N complexity hotspots.
   * What: Renders four stat cards (Quality grade, Hotspots count, Smells
   * count, Avg complexity) and a hotspots table. Auto-refreshes when the
   * selected index changes.
   * Test: Select an index on the Indexes page, return to Dashboard, confirm
   * stats populate.
   */
  import { onMount } from 'svelte';
  import {
    getSelectedIndex,
    getQuality,
    getHotspots,
    refreshQuality,
    refreshHotspots,
    refreshSmells
  } from '../state.svelte.js';
  import { navigate } from '../router.svelte.js';

  let selected = $derived(getSelectedIndex());
  let quality = $derived(getQuality());
  let hotspots = $derived(getHotspots());
  let loadError = $state(null);
  let loading = $state(false);

  async function loadAll(id) {
    if (!id) return;
    loadError = null;
    loading = true;
    try {
      await Promise.all([
        refreshQuality(id),
        refreshHotspots(id, 10),
        refreshSmells(id)
      ]);
    } catch (e) {
      loadError = e.message || String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if (selected) loadAll(selected);
  });

  // Re-load whenever the selected index changes after mount.
  let lastLoaded = $state('');
  $effect(() => {
    if (selected && selected !== lastLoaded) {
      lastLoaded = selected;
      loadAll(selected);
    }
  });

  function gradeClass(grade) {
    switch (grade) {
      case 'A':
        return 'grade-a';
      case 'B':
        return 'grade-b';
      case 'C':
        return 'grade-c';
      case 'D':
        return 'grade-d';
      case 'F':
        return 'grade-f';
      default:
        return 'grade-na';
    }
  }

  function fmtNum(n, digits = 1) {
    if (n === null || n === undefined) return '—';
    return Number(n).toFixed(digits);
  }
</script>

<div>
  <h1 class="page-title">Dashboard</h1>

  {#if !selected}
    <div class="card">
      <div class="card-body empty">
        <p>No index selected.</p>
        <button class="btn btn-primary" onclick={() => navigate('/indexes')}>
          Browse Indexes
        </button>
      </div>
    </div>
  {:else}
    {#if loadError}
      <div class="card mb-4" style="border-color: var(--trusty-danger)">
        <div class="card-body" style="color: var(--trusty-danger)">
          {loadError}
        </div>
      </div>
    {/if}

    <div class="stat-grid">
      <div class="stat">
        <div class="stat-label">Quality Grade</div>
        <div class="stat-value grade {gradeClass(quality?.grade)}">
          {quality?.grade || '—'}
        </div>
        <div class="stat-meta">overall</div>
      </div>
      <div class="stat">
        <div class="stat-label">Hotspots</div>
        <div class="stat-value">{hotspots?.length ?? '—'}</div>
        <div class="stat-meta">complexity outliers</div>
      </div>
      <div class="stat">
        <div class="stat-label">Smells</div>
        <div class="stat-value">{quality?.smell_count ?? '—'}</div>
        <div class="stat-meta">detected issues</div>
      </div>
      <div class="stat">
        <div class="stat-label">Avg Complexity</div>
        <div class="stat-value">{fmtNum(quality?.avg_cyclomatic)}</div>
        <div class="stat-meta">cyclomatic</div>
      </div>
    </div>

    <div class="card">
      <div class="card-header">Top 10 Complexity Hotspots</div>
      <div class="card-body" style="padding: 0">
        {#if loading}
          <div class="empty">Loading…</div>
        {:else if !hotspots || hotspots.length === 0}
          <div class="empty">No hotspots reported.</div>
        {:else}
          <table class="table">
            <thead>
              <tr>
                <th>Function</th>
                <th>File</th>
                <th>Cyclomatic</th>
                <th>Grade</th>
              </tr>
            </thead>
            <tbody>
              {#each hotspots.slice(0, 10) as h}
                <tr>
                  <td class="text-mono">{h.symbol || h.function || h.name || '—'}</td>
                  <td class="text-mono truncate" title={h.file || h.path}>
                    {h.file || h.path || '—'}
                  </td>
                  <td>{h.metrics?.cyclomatic ?? h.cyclomatic ?? '—'}</td>
                  <td>
                    <span class="badge {gradeClass(h.metrics?.grade ?? h.grade)}">
                      {h.metrics?.grade ?? h.grade ?? '—'}
                    </span>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .page-title {
    font-size: var(--trusty-fs-xl);
    font-weight: 600;
    margin: 0 0 var(--trusty-space-4) 0;
  }
  .grade {
    font-size: 2.5rem;
    line-height: 1;
  }
  .grade-a {
    color: var(--trusty-success);
  }
  .grade-b {
    color: var(--trusty-info);
  }
  .grade-c {
    color: var(--trusty-warning);
  }
  .grade-d {
    color: #f97316;
  }
  .grade-f {
    color: var(--trusty-danger);
  }
  .grade-na {
    color: var(--trusty-text-muted);
  }
</style>
